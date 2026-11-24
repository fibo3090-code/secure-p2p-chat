# Status — secure-p2p-chat
_Updated: 2026-11-24 18:45_
## In progress
- Nothing active. Branch `chore/two-peer-launcher` (1 commit, unpushed): `scripts/two-peers.ps1`, rebased on main
## Next
- Fix test isolation: `TuiApp::new` (`client/src/tui/app/mod.rs:171`) loads the REAL terminal profile (`%APPDATA%\chat-p2p\EncryptedMessenger`), so `the_first_run_password_prompt_cannot_be_dismissed` fails on any machine with an encrypted TUI identity, and the TUI tests read the real history
- Open a PR for `chore/two-peer-launcher`
- Remaining Nov-2026 audit findings (`docs/audit/2026-11`): NET-03, NET-04, TAURI-02/03/05/06/31, APP-02..09, SRV-07..19, CRYPTO-05/08..14, NET-19, QUAL-27
- Prune stale local branches: `fix/release-build`, `fix/release-stage-crlf`, `release/1.17.0`, `chore/repo-hygiene` (all already merged)
## Done (recent)
- 2026-11-24 #211 landed #197–#210 together: desktop UI redesign (#207, #210), CRYPTO-04 invite-mismatch warning (#209), rsa 0.10 (#203), rfd 0.18 + MSRV 1.92 (#202), 15% code trim (#206), dependency bumps
- 2026-11-24 Workspace checked on main c32c380: build ok, `npm test` 315/315, `cargo nextest` 683/684 (the one failure is the isolation bug above), `cargo deny check` clean
- 2026-11-10 v1.17.0 released (#193–#195)
## Blockers / waiting on the user
- Node 22.21 is below jsdom's requirement (>=22.22.2). Tests pass anyway; upgrade Node when convenient
- The website lives in a separate folder; this repo has no website files (the old `feat/website` branch was deleted)
