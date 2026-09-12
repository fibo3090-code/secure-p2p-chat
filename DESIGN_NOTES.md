# Design Notes

This document records the current UI/UX principles for the app and the main design constraints that contributors should preserve.

## Product Goals

- Keep the app understandable without requiring crypto expertise.
- Make security-critical actions visible and explicit.
- Prefer stable, simple workflows over visually clever ones.
- Avoid UI patterns that hide state changes or destructive outcomes.

## Current Interaction Principles

- Use short labels and direct actions.
- Show connection and trust state clearly.
- Confirm destructive operations.
- Keep advanced behavior behind settings or explicit dialogs.
- Treat fingerprints, addresses, and invite state as first-class UI data, not obscure technical strings.

## Security Signals

- Fingerprint verification must remain prominent.
- Trust state should remain easy to read.
- Password setup/unlock remains blocking by design.
- LAN discovery must not be presented as harmless or always-on.

## Brand and Theming

- The brand mark is a speech bubble carrying a two-node link glyph, in a
  teal-to-indigo gradient ("control teal-indigo", `#2dd4bf` → `#4f46e5`).
  The master source is `desktop/src-tauri/app-icon.svg`; every derived asset
  (icon trees, installer `.ico`, favicon, social preview) is regenerated from it.
- **`design/tokens.json` is the canonical source of record for colors**: the
  brand gradient, the flat UI accent, the five theme palettes (Light, Dark,
  Midnight, Forest, Rose), and the semantic colors (success/warning/error).
- Dark and Light carry the brand accent; Midnight, Forest, and Rose keep their
  own accent hues deliberately — they are alternate theme personalities, not
  brand-compliance failures.
- There is no cross-language token pipeline. The desktop app consumes the
  values as CSS custom properties (`desktop/src/app-system.css`,
  `desktop/src/themes.css`); the TUI uses the flat accent only for
  theme-neutral chrome. Each consumer has its own drift test against
  `design/tokens.json` — `desktop/src/lib/tokens.test.js` for the CSS
  properties and `token_drift_tests` in `client/src/tui/overlays.rs` for the
  TUI — so drift fails CI instead of going unnoticed.
- When changing brand colors: update `design/tokens.json` first, then each
  consumer; the drift tests then confirm the consumers agree.

## Frontends

There are two: the **Tauri + React desktop app** (`desktop/`) — the shipped
product, which realizes the designed tab-rail / list / content shell described in
`docs/platform_spec.md` §10 — and the **ratatui** TUI, for terminal and headless
use. An earlier egui desktop GUI existed and has been deleted; there is one app
on the releases page, so nobody has to guess which to install. Both frontends
drive the same `ChatManager`, so behavior stays consistent; the design intent
(one mental model, Party as a tab rather than a floating window, overlays only
for interruptive flows) is expressed most fully in the desktop app.

## Current Gaps

- Settings layout is still dense.
- File transfer progress/cancellation UX is still limited.
- Accessibility work is incomplete (the desktop app hand-rolls components; no headless a11y library yet).
- Parity between the desktop app and the TUI is functional, but not identical in polish; the TUI is deliberately narrower, covering the same actions through a typed command language rather than mirroring the layout.

## Design Change Rule

If a UI change affects:

- onboarding
- trust and verification
- destructive actions
- file transfer behavior
- settings defaults

then update this file and [docs/USER_GUIDE.md](docs/USER_GUIDE.md).
