# UI & UX Principles

## Core Principles
- Clarity: short labels, single purpose per control, predictable outcomes.
- Feedback: immediate state changes (connected, reconnecting, failed), toast + inline hints.
- Error prevention: validate IP:PORT, confirm destructive actions, default-safe toggles.
- Progressive disclosure: keep advanced settings behind accordions; show minimal by default.
- Security-first cues: fingerprint prominence, signed/verified badges, “we do not log” reminders.
- Consistency: shared spacing scale, typography ramp, color roles, component states.

## Visual Language
- Typography: Base 14–16px; headers +2–4 steps; monospace for addresses/fingerprints.
- Spacing: 4/8/12/16/24 grid; avoid zero padding in touch targets; min 36x36 hit areas.
- Color roles:
  - Success: #3FBF77
  - Warning: #E0A800
  - Error: #D9534F
  - Info/Neutral: #6C757D
  - Connectivity: Connected #3FBF77; Reconnecting #E0A800; Disconnected #D9534F
- Elevation: subtle shadows for dialogs; avoid heavy drop-shadows.
- Motion: prefer fades/scale-in; offer “reduced motion” toggle.

## Patterns by Area
- Onboarding / Invites:
  - Paste invite → auto-parse → show status + autofilled fields.
  - Reject malformed links with inline error; never prefill partial IP without port.
- Contacts:
  - Inline validation for IP:PORT and fingerprint length; success toast on save.
  - Display trust state badges (Unverified/Trusted/Blocked).
- Auto-host / Reconnect:
  - Persistent banner for current hosting port; explicit error if binding fails.
  - Reconnect status chip per contact; show next retry/backoff.
- Chat View:
  - Message bubble width 60–70% max; timestamps on hover or subtle inline.
  - Empty state with quick actions: “Connect”, “Invite friend”, “Start host”.
- File Transfer:
  - Progress bars with speed + ETA; warn on large files; allow cancel.
- Fingerprint Verification:
  - Two-step: show fingerprint + “copy” + color grid; require confirm/deny.

## Accessibility
- Contrast: target WCAG AA for text and controls.
- Focus: visible focus ring on all interactive elements; tab order matches visual order.
- Keyboard: Ctrl+Enter send, Esc close dialog, Ctrl+K quick switch, Enter default button.
- Text scaling: support 90–125% font scaling without layout breakage.
- Reduced motion: disable animations when requested.

## Trust & Safety Signals
- Show connection status near chat title.
- Toasts for host start/restart failures; inline errors for reconnect.
- Explicit fingerprint verification step; warn when peer fingerprint changes.
- Never log secrets; avoid showing private keys anywhere.

## Content Style
- Short, action-first labels (“Copy link”, “Retry connect”).
- Error messaging: cause + action (“Invalid IP: use host:port”).
- Avoid jargon in user-facing text; keep crypto details in Help.

