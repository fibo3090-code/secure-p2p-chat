# 07 — UI Refront: Tauri + SolidJS Rewrite Plan

> **Status:** Approved direction, pre-implementation. This document is the
> canonical plan for the total UI rewrite. No application code changes until the
> phases below are scheduled. Owner decisions captured: **tab rail per spec
> (P2P · Relay · Party)**, **Tauri shell**, **SolidJS + TypeScript frontend**,
> **full rewrite (no compatibility flag)**, **GUI + TUI redesigned to the same
> 3-pane mental model**.

## 1. Why

The current egui GUI is an accumulation, not a design: a top **menu bar**
(`Connection ▾ · Contacts · 🎉 Party · Settings · Help · 🔒 Lock`), a status bar,
a left **"Chats" sidebar**, a `CentralPanel`, a pile of modeless
`ActiveDialog` dialogs (`dialogs.rs` alone is ~84 KB), and — the clearest
symptom — **Party lives in a separate floating `egui::Window`**, disconnected
from everything else. Users juggle two mental models glued together.

`docs/05_platform_spec.md` already states the north star: **"one app, one
identity, a left tab rail (Discord-like): P2P · Relay · Party."** The current
GUI does not implement it. This rewrite realizes that spec and replaces the
immediate-mode + dialog-soup shell with a retained, designed, animated UI.

## 2. Target shape — a 3-pane shell

```
┌──┬───────────────┬─────────────────────────────┐
│ R│   List pane   │      Content pane           │
│ a│ (chats /      │  (messages + composer, OR   │
│ i│  channels +   │   directory, OR a settings  │
│ l│  members /    │   page — no floating modals)│
│  │  relay sess.) │                             │
└──┴───────────────┴─────────────────────────────┘
 icon rail (top→bottom): P2P · Relay · Party · … · identity/lock/settings
```

- **Icon rail** replaces the menu bar; switches *mode* (P2P / Relay / Party) per
  spec. Identity, lock, and settings dock at the bottom.
- **List pane** is contextual to the mode (peer chats / server channels+members /
  relay sessions).
- **Content pane** holds the conversation **or** a full-pane view for directory
  and settings — eliminating most modal dialogs.
- **Overlays only for genuinely interruptive flows:** fingerprint verification,
  password/unlock, consent-or-leave. Everything else (Settings, Contacts,
  Connect, Host) becomes an inline page/panel.
- **Party stops being a floating window** and becomes the Party tab, sharing the
  exact list+content layout as P2P. One mental model.

## 3. Stack decision & rationale

| Layer | Choice | Why |
|---|---|---|
| Shell | **Tauri 2** | Native window + system webview, Rust core stays behind an IPC boundary, capability/CSP security model (tighter than Electron), no bundled Node runtime. |
| Frontend | **SolidJS + TypeScript** | Fine-grained reactivity, *no VDOM diffing* → best render perf for a high-frequency message stream; smallest runtime → fastest webview cold-start. |
| Styling | **Tailwind CSS** + design tokens | Real token system; replaces ad-hoc `styling.rs`. Carries Light/Dark/Midnight/Forest themes as token sets. |
| Components | **Kobalte** (a11y headless) + **Motion One** (animation) + **TanStack Virtual** (message list virtualization) | Discord-grade polish without hand-rolling primitives. |
| Build | **Vite** | Standard Solid toolchain; `tauri dev`/`tauri build` orchestrate it. |

**Accepted costs (explicitly owned):**
1. A JS/TS toolchain (Node + pnpm + Vite) joins a previously pure-Rust repo.
   Contributors need both toolchains.
2. The TUI (ratatui) remains Rust and is **redesigned in parallel** to the same
   3-pane model — two frontends of the same app are maintained.
3. **Packaging is rebuilt from scratch** (see §7) — arguably the largest cost.
4. **Security surface** grows by a system webview + Tauri IPC. Crypto stays in
   Rust behind commands; `SECURITY.md` must document the webview + CSP posture.
5. Full rewrite = an intentional window where the GUI does not build/run; the
   **TUI is the working frontend** during that window.

## 4. Architecture — the Rust↔web bridge

The business logic is **already decoupled from the view** — the existing TUI
proves it by driving the same `ChatManager` / `PartyManager` as the GUI. The
rewrite keeps those managers in Rust and exposes them to the web UI:

- **Commands** (`#[tauri::command]`): every manager method the UI calls becomes a
  command — `connect`, `host`, `send_message`, `set_connection_password`,
  `set_conversation_locked`, `confirm_fingerprint`, `party_join`, `party_post`,
  `party_send_dm`, `party_create_channel`, `party_fetch_history`, etc.
- **Events** (`app_handle.emit`): the existing `SessionEvent` mpsc stream maps
  almost 1:1 onto Tauri's event channel. A single background task drains
  `poll_session_events()` / party `poll_events()` and emits typed events
  (`session://event`, `party://event`) the Solid frontend subscribes to. This
  replaces per-frame `try_lock()` polling with push — a better fit for the
  domain.
- **Shared types:** define DTOs once in Rust (`serde`) and generate matching TS
  types (e.g. `ts-rs` or `tauri-specta`) so the IPC contract can't silently
  drift. The CLAUDE.md symmetry discipline for `to_plain_bytes` /
  `from_plain_bytes` is unaffected — wire framing stays in `core`.

```
core/ (unchanged: crypto, protocol, identity, transport)
client/
├── src-tauri/        ← Rust: managers + #[tauri::command] + event pump + main.rs
│   └── (keeps ChatManager, PartyManager, persistence; deletes src/gui/)
└── ui/               ← SolidJS app: rail / list / content + overlays + design tokens
```

The `--tui` path and the TUI module stay in the Rust binary (or a sibling bin);
launching `client` opens the Tauri window, `client --tui` opens the ratatui UI.

## 5. Scope of deletion / preservation

**Delete:** `client/src/gui/` entirely (`app_ui.rs`, `chat_view.rs`,
`dialogs.rs`, `help_view.rs`, `party_view.rs`, `sidebar.rs`, `styling.rs`,
`widgets.rs`), and the `eframe`/`egui`/`egui_commonmark`/`egui_tracing`
dependencies once nothing references them.

**Preserve untouched:** `core/`, `server/`, `client/src/app/*` (managers,
persistence), `client/src/identity/*`, all of `core`'s network/crypto/protocol.
**Preserve & redesign:** `client/src/tui/*` (new 3-pane layout, same command
language).

## 6. TUI parallel redesign

Mirror the 3-pane model in ratatui: a left **rail** (P2P/Relay/Party as a
vertical list or number-keyed modes), a **list pane**, a **content pane**, with
the existing typed-command language preserved. This keeps GUI and TUI on one
mental model and prevents the drift that exists today. The command set in
`client/src/tui/command.rs` is the contract; layout is what changes.

## 7. Packaging rebuild (the hidden cost)

Current pipeline assumes one `eframe` binary named `encodeur_rsa_rust`:
- `release.yml` builds `-p encodeur_rsa_rust`, then hand-rolls Inno Setup
  (`setup.iss`), a macOS `.app`+`.dmg`, and a Linux tarball.
- `build-and-package.ps1` and `setup.iss` hardcode the binary name.

Tauri replaces all of this with its **own bundler** (`tauri build` →
`.msi`/`.nsis` on Windows, `.app`/`.dmg` on macOS, `.deb`/`.AppImage` on Linux)
and the **`tauri-action`** GitHub Action. Tasks:
- Add `tauri.conf.json` (identifier `com.fibo3090.messenger`, window 1200×800
  min 800×600 to match today, icons from `encodeur_rsa_icon.ico`).
- Rewrite `release.yml` to install Node + Rust, run `tauri-action` per-OS,
  attach Tauri-built artifacts to the tag release. Keep the `on: push: tags:
  'v*'` trigger and the "ensure release exists" behavior.
- Retire or repurpose `setup.iss` and `build-and-package.ps1` (Tauri's NSIS/MSI
  bundlers supersede Inno Setup). Document the change.
- `ci.yml` gains a frontend job: `pnpm install`, typecheck, lint, build; keep
  `cargo fmt/clippy/test/build --locked` for the Rust side.

## 8. Phased execution (within the "full rewrite" decision)

The owner declined a compatibility flag, so there is no parallel old-GUI period —
but the work is still sequenced so each phase is reviewable and the binary keeps
building (TUI remains functional throughout):

- **Phase A — Scaffold & bridge.** Add Tauri 2 + Solid/Vite to `client/`,
  restructure into `src-tauri/` + `ui/`, stand up an empty window, wire the
  command/event bridge for one read path (e.g. list chats) and one event
  (`session://event`). Generate TS types. CI builds both toolchains.
- **Phase B — P2P tab end-to-end.** Rail → chat list → message view (virtualized)
  → composer → **fingerprint-verify overlay** → connect/host as inline pages.
  Lock/password flows. This is the proof that the bridge + design hold.
- **Phase C — Party tab.** Server join, channel list + members, channel messages,
  DMs, channel creation, presence — reusing the list+content layout. Deletes the
  floating-window model conceptually.
- **Phase D — Relay tab + Settings/Contacts/Help pages.** Move the remaining
  dialog-soup surfaces into inline pages; themes via Tailwind tokens.
- **Phase E — Delete egui.** Remove `client/src/gui/` and the egui deps; switch
  `main.rs` GUI launch to Tauri.
- **Phase F — Packaging + docs.** Rewrite `release.yml` to `tauri-action`, retire
  `setup.iss`/`.ps1`, update `05_platform_spec.md` (egui→Tauri/Solid), add a
  webview/CSP section to `SECURITY.md`, refresh `USER_GUIDE.md`/`TUTORIAL.md`.
- **Phase G — TUI 3-pane redesign.** Apply the rail/list/content model to
  ratatui.

## 9. Per-phase verification

- **A:** `cargo build` + `pnpm build` green in CI; window opens; one command +
  one event round-trip works; `client --tui` still runs.
- **B:** Two LAN clients connect via the new UI, fingerprint overlay verifies,
  messages flow, lock/password enforced. Render-no-crash on empty + populated
  states.
- **C:** Two clients join a Party server through the new UI, exchange channel +
  DM messages, create a channel, see directory/presence — mirrors the existing
  `serve_connection` E2E coverage.
- **D/E:** No `egui`/`eframe` symbols remain (`grep` clean); all former dialogs
  reachable as pages; themes switch live.
- **F:** `tauri build` produces installers on all three OSes in CI; tagging
  `vX.Y.Z` attaches them; docs no longer reference egui.
- **G:** TUI renders the 3-pane layout; existing TUI command tests stay green.

## 10. Risks & open items

- **Compounding risk:** Tauri + full rewrite + TUI redesign are three large
  changes. Phasing (above) keeps the binary building and the TUI usable
  throughout; Phase B is the early go/no-go on the whole approach.
- **IPC drift:** mitigated by generating TS types from Rust DTOs.
- **Webview variance:** WebView2 / WKWebView / WebKitGTK render differently;
  budget cross-platform QA. Linux CI needs `libwebkit2gtk` dev packages.
- **Version bump:** this is a major UX change — plan a `2.0.0` when Phase F lands
  (owner to authorize the tag, per release-on-tag gating).
- **Spec amendment:** `05_platform_spec.md` currently assumes egui; update it in
  Phase F so the canonical spec and the shipped stack agree.
