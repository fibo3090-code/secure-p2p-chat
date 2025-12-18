# Design & UI/UX Guide

> **Comprehensive design documentation for the Encrypted P2P Messenger**

This document consolidates UI/UX principles, design patterns, component specifications, and implementation roadmap.

---

## Table of Contents

1. [Core Principles](#core-principles)
2. [Visual Language](#visual-language)
3. [Information Architecture](#information-architecture)
4. [Key User Flows](#key-user-flows)
5. [Component Kit](#component-kit)
6. [Design Patterns by Area](#design-patterns-by-area)
7. [Accessibility](#accessibility)
8. [Trust & Security Signals](#trust--security-signals)
9. [Content Style Guide](#content-style-guide)
10. [Design Roadmap](#design-roadmap)

---

## Core Principles

- **Clarity**: Short labels, single purpose per control, predictable outcomes
- **Feedback**: Immediate state changes (connected, reconnecting, failed), toast + inline hints
- **Error Prevention**: Validate IP:PORT, confirm destructive actions, default-safe toggles
- **Progressive Disclosure**: Keep advanced settings behind accordions; show minimal by default
- **Security-First Cues**: Fingerprint prominence, signed/verified badges, "we do not log" reminders
- **Consistency**: Shared spacing scale, typography ramp, color roles, component states

---

## Visual Language

### Typography

- **Base**: 14–16px for body text
- **Headers**: +2–4 steps from base (18px, 20px, 24px)
- **Monospace**: For addresses, fingerprints, and technical strings
- **Line Height**: 1.5 for body, 1.2 for headers

### Spacing Scale

- **Grid**: 4/8/12/16/24px increments
- **Dense Lists**: 8px padding
- **Dialogs**: 16–20px padding
- **Touch Targets**: Minimum 36x36px hit areas
- **Zero Padding**: Avoid in touch targets

### Color Roles

- **Success**: `#3FBF77` (green)
- **Warning**: `#E0A800` (amber)
- **Error**: `#D9534F` (red)
- **Info/Neutral**: `#6C757D` (gray)
- **Connectivity States**:
  - Connected: `#3FBF77` (green)
  - Reconnecting: `#E0A800` (amber)
  - Disconnected: `#D9534F` (red)

### Elevation & Depth

- **Subtle Shadows**: For dialogs and cards
- **Avoid Heavy Drop-Shadows**: Keep it minimal and modern

### Border Radius

- **Cards/Bubbles**: 6–10px
- **Inputs/Buttons**: 4px

### Motion & Animation

- **Preferred**: Fade/scale-in for dialogs, slide/fade for toasts
- **Respect Reduced Motion**: Offer toggle in settings
- **Duration**: 150-300ms for most transitions

---

## Information Architecture

- Top navigation: Chats | Contacts | Settings | Help.
- Global status bar: connection/host state, fingerprint copy, quick reconnect.
- Panels:
  - Left: chat list + contacts toggle + search/quick switch.
  - Main: message view, composer, status banners/toasts.
  - Dialogs: Connect, Host, Invite, Fingerprint verification, File send.

## Key User Flows

- Invite paste:
  - Field + “Paste” → parse → show “Link valid” + autofill name/address/fingerprint → inline error if bad.
- QR Code Scan:
  - Button to open camera → scan QR code → parse invite link → autofill contact details.
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

## Component Kit

- Buttons: primary/secondary/ghost; states: default, hover, active, disabled, loading.
- Inputs: text, multiline, chip selector; validation states with helper text.
- Chips/Badges: trust states, statuses (connected/reconnecting/error).
- Toasts/Banners: success/warn/error/info; with optional action link.
- Tabs: for Contacts (Manual / Invite Link / Share Link), Settings sections.
- Cards: for identity summary, connection info, file items.
- Progress: linear bars for file transfer and reconnect backoff.
- Icons: lightweight line icons; keep size consistent (16–20px).

---

## Design Patterns by Area

### Onboarding / Invites

- **QR Code Sharing**: Display a QR code for the invite link to be scanned by another device.
- **Paste Invite**: Auto-parse → show status + autofilled fields
- **Validation**: Reject malformed links with inline error
- **Safety**: Never prefill partial IP without port

### Contacts Management

- **Inline Validation**: For IP:PORT format and fingerprint length (64 hex chars)
- **Success Feedback**: Toast notification on save
- **Trust State Badges**: Unverified / Trusted / Blocked
- **Required Fields**: Name (required), Address (optional/validated), Fingerprint (64 hex)

### Auto-Host / Reconnect

- **Persistent Banner**: Show current hosting port with "Copy address" action
- **Error Handling**: Explicit error toast if binding fails with retry action
- **Reconnect Status**: Chip per contact showing Connected / Reconnecting / Failed
- **Backoff Display**: Show next retry time

### Chat View

- **Message Bubbles**: 60–70% max width
- **Timestamps**: On hover or subtle inline
- **Empty State**: Quick actions - "Connect", "Invite friend", "Start host"
- **Status Indicators**: Connection state near chat title

### File Transfer

- **Progress Bars**: Show speed + ETA
- **Size Warnings**: Alert on large files (approaching 2GB limit)
- **Cancel Option**: Allow cancellation mid-transfer
- **Dropzone**: Visual feedback on drag-over

### Fingerprint Verification

- **Two-Step Process**:
  1. Show fingerprint + "Copy" button + color grid visualization
  2. Require explicit "Trust" or "Reject" action
- **Change Warnings**: Alert when peer fingerprint changes
- **No Auto-Accept**: Always require user confirmation

---

## Accessibility

### Contrast & Visibility

- **Target**: WCAG AA compliance for all text and controls
- **Focus Indicators**: Visible focus ring on all interactive elements
- **Tab Order**: Matches visual order logically

### Keyboard Navigation

- **Ctrl+Enter**: Send message
- **Esc**: Close dialog
- **Ctrl+K**: Quick switch (planned)
- **Enter**: Activate default button
- **Tab/Shift+Tab**: Navigate between controls

### Responsive Design

- **Text Scaling**: Support 90–125% font scaling without layout breakage
- **Reduced Motion**: Disable animations when system preference set
- **Screen Readers**: Proper ARIA labels (future enhancement)

---

## Trust & Security Signals

### Visual Security Cues

- **Connection Status**: Always visible near chat title
- **Fingerprint Prominence**: Easy to copy and verify
- **Verification Badges**: Clear trust state indicators
- **Change Warnings**: Alert on fingerprint changes

### Error Communication

- **Host Failures**: Toast notifications with retry action
- **Reconnect Issues**: Inline errors with status chips
- **Validation Errors**: Inline with clear cause + action

### Privacy Protection

- **No Secret Logging**: Never log private keys or passwords
- **No Secret Display**: Avoid showing private keys in UI
- **Secure Defaults**: All security features enabled by default

---

## Content Style Guide

### Writing Principles

- **Action-First Labels**: "Copy link", "Retry connect", "Send message"
- **Clear Error Messages**: Cause + action ("Invalid IP: use host:port format")
- **Avoid Jargon**: Keep crypto details in Help/Documentation
- **Positive Framing**: "Connected" not "Not disconnected"

### Tone

- **Friendly but Professional**: Approachable without being casual
- **Concise**: Short sentences, clear meaning
- **Helpful**: Guide users to success
- **Honest**: Transparent about limitations

### Microcopy Examples

- ✅ "Listening on port 8080" (not "Host active")
- ✅ "Reconnecting in 5s..." (not "Retry pending")
- ✅ "Invalid IP: use host:port" (not "Error: bad input")
- ✅ "Copy invite link" (not "Get link")

---

## Design Roadmap

### Phase 1: Quick Wins (1–2 weeks)

- ✅ Improve invite/contact dialogs with inline validation
- ✅ Add connection/host status banner and reconnect chips
- ⏳ Polish toasts (actions + consistent colors)
- ⏳ Enhance empty states with quick actions

### Phase 2: Core UX (3–4 weeks)

- 🔄 Fingerprint verification flow refresh
- 🔄 File transfer progress UX improvements
- 📋 Quick switch (Ctrl+K) for chats/contacts
- 📋 Better search/filter functionality
- 📋 Reduced motion + font scale handling
- 📋 WCAG AA contrast audit

### Phase 3: Polish & Enhancement (Later)

- 📋 Theme refinement (light/midnight modes)
- 📋 Icon pass for consistency
- 📋 Microcopy polish across all surfaces
- 📋 mDNS/peer discovery UI
- 📋 Richer notification system
- 📋 Advanced accessibility features

**Legend**: ✅ Done | ⏳ In Progress | 🔄 Next Up | 📋 Planned

---

## Implementation Notes

### For Developers

- All color values should use constants from a central theme file
- Spacing should use the defined grid system (4/8/12/16/24)
- Component states should be consistent across the application
- Accessibility features should be built-in, not added later

### For Designers

- Maintain design system consistency
- Test with actual content, not lorem ipsum
- Consider edge cases (long names, many contacts, etc.)
- Validate designs with security team for trust signals

### Testing Checklist

- [ ] Keyboard navigation works for all flows
- [ ] Focus indicators visible on all interactive elements
- [ ] Color contrast meets WCAG AA standards
- [ ] Text scales properly at 125%
- [ ] Reduced motion preference respected
- [ ] Error states provide clear guidance
- [ ] Success states provide clear feedback

---

**Related Documentation:**

- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) - Technical implementation
- [SECURITY.md](SECURITY.md) - Security requirements
- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines
