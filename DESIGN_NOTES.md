# Design Notes (Working Doc)

## Information Architecture
- Top navigation: Chats | Contacts | Settings | Help.
- Global status bar: connection/host state, fingerprint copy, quick reconnect.
- Panels:
  - Left: chat list + contacts toggle + search/quick switch.
  - Main: message view, composer, status banners/toasts.
  - Dialogs: Connect, Host, Invite, Fingerprint verification, File send.

## Key Flows (textual wireframes)
- Invite paste:
  - Field + “Paste” → parse → show “Link valid” + autofill name/address/fingerprint → inline error if bad.
- Add contact:
  - Name (required), Address IP:PORT (optional/validated), Fingerprint (64 hex), Notes/Tags; success toast.
- Auto-host:
  - Toggle in Settings → banner “Listening on :<port>” with “Copy address”.
  - On failure: error toast + retry action.
- Auto-reconnect:
  - Status chip per contact: Connected / Reconnecting / Failed; “Retry now” button.
- File send:
  - Dropzone/button → show filename, size, estimated time; progress + cancel; warning on oversize.
- Fingerprint verify:
  - Show peer name, fingerprint, color grid; actions: “Trust”, “Reject”, “Copy”.

## Component Kit (starter)
- Buttons: primary/secondary/ghost; states: default, hover, active, disabled, loading.
- Inputs: text, multiline, chip selector; validation states with helper text.
- Chips/Badges: trust states, statuses (connected/reconnecting/error).
- Toasts/Banners: success/warn/error/info; with optional action link.
- Tabs: for Contacts (Manual / Invite Link / Share Link), Settings sections.
- Cards: for identity summary, connection info, file items.
- Progress: linear bars for file transfer and reconnect backoff.
- Icons: lightweight line icons; keep size consistent (16–20px).

## Visual Tokens
- Spacing: 4/8/12/16/24 grid; dense lists 8px padding, dialogs 16–20px.
- Typography: Base 14–16px; headings +2/+4 steps; monospace for technical strings.
- Colors: see `docs/ui_ux_principles.md`; keep status colors consistent across banners/toasts/chips.
- Radius: 6–10px for cards/bubbles; 4px for inputs/buttons.
- Motion: fade/scale-in for dialogs; slide/fade toasts; respect reduced motion.

## Accessibility & Trust Checklist (summary)
- WCAG AA contrast for text and controls.
- Visible focus ring; tab order logical; keyboard shortcuts documented.
- Reduced motion option; text scaling resilient.
- Security cues: fingerprint shown with copy; warning on change; no secrets in logs.

## Roadmap (priority)
- Quick wins (1–2 weeks):
  - Improve invite/contact dialogs with inline validation + clearer success/error.
  - Add connection/host status banner and reconnect chips.
  - Polish toasts (actions + consistent colors) and empty states.
- Next (3–4 weeks):
  - Fingerprint verification flow refresh; file transfer progress UX.
  - Quick switch (Ctrl+K) and better search/filter for chats/contacts.
  - Reduced motion + font scale handling; audit contrast.
- Later:
  - Theme refinement (light/midnight), icon pass, microcopy polish.
  - mDNS/peer discovery surfaces, richer notifications.

