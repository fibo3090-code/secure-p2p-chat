# External audit thread (user-provided, revision e7dd9e3, static only)

Findings reported by a separate review thread, to be cross-checked during consolidation.

## Security / reliability
- EXT-01 (High): a peer can initiate key rotations reserved to the host. `initiate_rekey` comment says only the host rotates, but the receiver accepts any valid `Rekey` without checking the sender's role. An authenticated client could trigger repeated rotations / desync. core/src/network/session.rs (initiate_rekey ~1269, Rekey receipt ~1523 at e7dd9e3).

## Backup & recovery
- EXT-02 (High): "backup identity and history" export only copies identity.json; history/contacts live in history.json.enc. Welcome screen promises identity+history. desktop/src-tauri/src/commands/auth.rs:201, lib.rs:400, desktop/src/App.jsx:268.
- EXT-03 (Medium): UI explains how to restore "over the file" but no restore command exists. desktop/src/components/Onboarding.jsx:47, auth.rs:201.
- EXT-04 (Medium): backup reminder is dismissed after any export to an arbitrary destination; measures that an export happened, not that a recoverable copy exists. Overwrite confirmation not verified. auth.rs:216, :228.
- EXT-05 (gap): no flow to delete the identity / all local data in the desktop app; only per-conversation delete. desktop/src-tauri/src/commands/chats.rs:558.

## Documentation
- EXT-06 (High): desktop/src-tauri/Cargo.toml:26 tech-debt note still mentions egui/ratatui deps and extracting ChatManager — stale.
- EXT-07 (High): design/tokens.json:2 points to deleted client/src/gui/styling.rs and a `token_drift_tests` test not found; platform_spec says egui was removed (docs/platform_spec.md:104).
- EXT-08 (Medium): docs/platform_spec.md:42 "current state" verified at 1.15.0 while workspace/README are 1.16.2.

## Developer workflow
- EXT-09 (optimisation): frontend build/test/lint/smoke run on all three OSes (ci.yml:395); full suite on one OS + OS-specific checks elsewhere could cut CI time.
- EXT-10 (friction): no single local equivalent of CI; CONTRIBUTING.md:22 vs DEVELOPER_GUIDE.md:71 differ (nextest + doctests vs cargo test). Provide one copy-pasteable command (nextest --profile ci + doctests).

## Hosting / lifecycle (second batch, static, revision 62b952f)
- EXT-11 (Medium): connecting to one's own identity is not refused at the conversation level. ChatManager knows the local fingerprint (`set_my_fingerprint`, chat_manager/mod.rs:~281) but only import_contact uses it; a direct self-connection is treated as a new peer and can create both an incoming and an outgoing conversation (events.rs:~175). Explains "two strange conversations".
- EXT-12 (Medium): deleting the host placeholder conversation does not stop hosting. `delete_chat` (desktop commands/chats.rs:~558) leaves `is_hosting`/`hosting_port` set; the 1.5 s rehost check (desktop lib.rs:~578/606, chat_manager/mod.rs:~461 `check_rehost_needed`) sees "hosting but no placeholder" and re-listens ("auto-rehosted after a consumed listener"). Intended after a peer consumes the listener; wrong when the user deletes the placeholder to stop hosting. `stop_hosting` (mod.rs:~478) is the path that clears the state.
- EXT-13 (Medium): `clear_history` (chat_manager/mod.rs:~748) empties chats and stops session tasks but does not reset `is_hosting`/`hosting_port`, so the periodic check can recreate a placeholder and reopen the port after "clear everything". Needs a product decision (should clearing stop hosting?).
- EXT-14 (Medium): unlocking (the "lock" switch, not identity unlock) never resumes hosting although the code comment says rehost resumes: lock calls `stop_hosting()` (desktop commands/connect.rs:~113) which clears is_hosting/port; unlock only clears the lock flag, so `check_rehost_needed` (mod.rs:~461) can never restart the host. Test `lock_stops_hosting_and_unlock_allows_rehost` (desktop src-tauri/src/tests.rs:~636) only asserts the lock flag, not that hosting resumes.
- Summary: three paths (EXT-12, EXT-13, EXT-14) where displayed and real hosting state diverge, plus the self-connection case (EXT-11).
