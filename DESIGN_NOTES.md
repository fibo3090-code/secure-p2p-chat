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

## Current Gaps

- Settings layout is still dense.
- File transfer progress/cancellation UX is still limited.
- Accessibility work is incomplete.
- GUI/TUI parity is functional, but not identical in polish.

## Design Change Rule

If a UI change affects:

- onboarding
- trust and verification
- destructive actions
- file transfer behavior
- settings defaults

then update this file and [docs/USER_GUIDE.md](docs/USER_GUIDE.md).
