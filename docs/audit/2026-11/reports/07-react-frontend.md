# React/Vite desktop web UI (desktop/src)

## Scope covered

Read in full: `desktop/src/App.jsx`, `main.jsx`, `index.html`, `vite.config.js`, `eslint.config.js`, `package.json`,
every component (`Messages.jsx`, `Parties.jsx`, `PartyAdmin.jsx`, `Creator.jsx`, `Contacts.jsx`, `Settings.jsx`,
`Relays.jsx`, `Onboarding.jsx`, `Verify.jsx`, `ChatDialogs.jsx`, `Toasts.jsx`, `SafetyGrid.jsx`, `ui.jsx`),
every lib module (`bridge.js`, `toast.js`, `partyUnread.js`, `localStore.js`, `themes.js`, `csp.js`, `parse.js`,
`password.js`, `colorgrid.js`, `Icon.jsx`, `invokeContract.js`), the test helpers (`src/test/*`), the smoke specs
(`desktop/smoke/*`), the CSS (`styles.css`, `app-system.css`, `polish.css`, `themes.css`) and `design/tokens.json`.
Component test files were skimmed for what they cover.

Cross-checked against the Rust side where the UI contract matters: `desktop/src-tauri/src/lib.rs` (poll loop,
`ui_signature`), `commands/party.rs` (`party_join`, file DTO), `commands/chats.rs` (`get_conversation`,
`file_preview`), `commands/contacts.rs`, `client/src/app/party_manager.rs` (`connect_and_join`, server ids).

Run read-only: `npx vitest run` (24 files, 287 tests, all pass) and `npx eslint .` (clean, no warnings). One
throwaway jsdom script in the scratchpad confirmed the Modal autofocus behaviour (UI-08). The current tree
includes PR #188 (the preview fix `ec51a96`); the preview-cancellation path in `Messages.jsx` was judged as it
stands now and is correct.

## Summary

The frontend is careful in many places and the comments explain why. Examples: bridge-backed unread, the
undismissable TOFU modal with an escape hatch, the risky-file confirmation, strict port parsing, CSP drift tests,
and stub-from-real-api tests. No XSS sinks exist: there is no `dangerouslySetInnerHTML`, peer text renders as React
text, links go through `open_url`, and SVG previews are refused.

The main problems are about state and async ordering, plus one contract gap with security impact:

1. **High (security):** first-join verification for a community does not cover the connection that carries the
   credentials. The UI shows the SAS from connection #1. Then it calls `party_join(..., trust=true)` without the
   fingerprint the user just checked, so connection #2 trusts whatever key answers it (UI-01).
2. **High (bug):** a late `get_conversation` response can put conversation A on screen while `activeId` (which the
   composer sends to) is B. Text typed while looking at A then goes to B (UI-02). Parties has the same class of
   bug: stale history responses (UI-04) and one composer draft shared across every channel, DM and server (UI-03).
3. Community read marks are keyed by a server UUID that is regenerated on every join, so the persistence they were
   written for never works across a restart (UI-05).
4. Every `state-updated` refetches and re-maps the whole open conversation. That is up to 4 Hz during any file
   transfer (UI-06).
5. Several accessibility and UX issues:
   - autofocus is silently defeated in every modal (UI-08)
   - the toast live regions may not announce (UI-09)
   - `--faint` text fails contrast (UI-10)
   - the invite link and relay tokens go stale or are lost (UI-07, UI-14)

## Findings, ordered by severity

### UI-01: Community first-join SAS check does not cover the connection that receives the credentials
- Severity: High
- Category: security
- Status: VERIFIED
- Location: `desktop/src/components/Parties.jsx:62-80`, `:101-102`; `desktop/src/lib/bridge.js:167-168`; `desktop/src-tauri/src/commands/party.rs:332-346`; `client/src/app/party_manager.rs:404-428`
- Problem: Step 1 (`trust=false`) opens connection #1, returns `{status:"verify", fingerprint, sas}` and hangs up.
  The verify card stores `verify.fingerprint`. "The code matches — join" then calls
  `joinWith(verify.address, verify.username, verify.password, true)`. The fingerprint is never sent back:
  `api.partyJoin` has no parameter for it, and `party_join` only uses a pin from `parties.json`, which does not
  exist yet on a first join. `connect_and_join` opens a **new** TCP and handshake (connection #2). With
  `expected_fingerprint == None` and `trust_new_identity == true` it reaches the `None => {}` arm, which accepts any
  key, and then writes the `Join` frame carrying username and password.
- Impact: An on-path attacker can let connection #1 through untouched, so the user sees and confirms the real
  operator's SAS. The attacker then terminates connection #2 with its own key and receives the community
  credentials. The attacker's key is also pinned into `parties.json` from then on. This defeats the two-step trust
  design that CLAUDE.md describes ("no credential moves before step two").
- Recommendation:
  - Pass the fingerprint the user confirmed on step 2, e.g. `partyJoin(address, username, password, trust, fingerprint)`.
  - In `party_join`, when `trust` is true, use that fingerprint as `expected_fingerprint` so a mismatch bails
    before `Join` is written.
  - Add a bridge test where the two connections present different keys, and a `Parties.test.jsx` assertion that
    the confirm click sends the verified fingerprint.
  - If the second handshake's key differs, show "the server's identity changed between checks".
- Effort: S

### UI-02: A late conversation fetch can show A while the composer sends to B
- Severity: High
- Category: bug
- Status: VERIFIED (missing guard). Whether it triggers depends on timing.
- Location: `desktop/src/App.jsx:209-216` (`refresh`), `:284-299` (`openConv`), `:301-310` (`send`), `:403-404`
- Problem: Both `refresh()` and `openConv()` do `api.getConversation(id).then(chat => setActive(chatToContact(chat, …)))`
  with no check that `id` is still the open conversation. If a refresh for A, or an earlier `openConv(A)`, resolves
  after `openConv(B)`, `active` becomes A while `activeId` and `activeIdRef` stay B. `ChatPane` renders `contact`
  (A's header, messages and name in the placeholder), but `send()` posts `draft` to `activeId` (B).
  `sendFile(c)` uses `contact.id` (A), so the two actions even disagree. Because `state-updated` only fires when
  `ui_signature` changes (`lib.rs:455-484`), an idle app can stay in this state indefinitely.
  `get_conversation` waits on the manager lock (`chats.rs:194`), so out-of-order completion is plausible under load
  or during rapid clicks.
- Impact: A message the user composed while reading conversation A is delivered to peer B. That is a
  confidentiality failure in a secure messenger.
- Recommendation:
  - In both `.then` callbacks, write only if `activeIdRef.current === id`.
  - Better, derive the composer's target from `active.id`, and refuse to send when `active?.id !== activeId`.
  - Move the side effect out of the `setActiveId` updater: StrictMode calls updaters twice in dev, so every refresh
    issues the IPC twice.
  - Add a test with deferred promises resolving out of order.
- Effort: S

### UI-03: Communities composer draft is shared across channels, DMs and servers
- Severity: Medium
- Category: bug / security
- Status: VERIFIED
- Location: `desktop/src/components/Parties.jsx:208`, `:324-333`, `:492`, `:511-513`, `:557-560`
- Problem: `draft` is a single state in `Parties`. Switching channel, DM or server never clears or stores it. Text
  typed in a private DM stays in the box after clicking a public channel, and Enter posts it there. `send()` also
  restores a failed draft with `setDraft(text)` into whatever thread is open when the promise rejects. `App.jsx:292-294`
  clears drafts on switch for exactly this reason ("so unsent text can't follow the user into another thread and be
  sent to the wrong recipient"); Parties does not.
- Impact: A message reaches the wrong audience, for example a DM's content posted to a public channel of the whole
  community.
- Recommendation: Keep drafts per thread key (`${sid}|${cid|dm-x}`) in a map, or clear on every thread change.
  Restore a failed draft only if the thread key is unchanged.
- Effort: S

### UI-04: Stale Communities history responses can render under the wrong channel header
- Severity: Medium
- Category: bug
- Status: VERIFIED
- Location: `desktop/src/components/Parties.jsx:245-252`, `:272-276`
- Problem: `loadMsgs` is a callback over `(sid, cid, dm)` and calls `setMsgs(await …)` unconditionally. Every
  `party-updated` event and every selection change starts a fetch, and a slower earlier fetch for channel X can
  resolve after the fetch for channel Y. Its messages then show under `# Y` until the next event.
- Impact: A private channel's or DM's history can be shown under another thread's name, which misleads screenshots
  and replies. Combined with UI-03, users reply in the wrong place.
- Recommendation: Tag each request with the thread key or a sequence counter, and drop results for a key that is no
  longer active (`useRef` holding the current key).
- Effort: S

### UI-05: Community read marks never survive a restart or rejoin
- Severity: Medium
- Category: bug
- Status: VERIFIED
- Location: `desktop/src/lib/partyUnread.js:58`, `:90-101`, `:105-115`; `desktop/src/components/Parties.jsx:239`; `client/src/app/party_manager.rs:504`
- Problem: Marks are keyed `${server.id}|…`. `server.id` is `Uuid::new_v4()`, minted on every `connect_and_join`.
  After a restart or rejoin every key is new, so `computeUnread` reports the whole channel history as unread ("no
  stored mark counts as fully unread"). `pruneTo(list)` then deletes the old marks: their server ids are not live.
  If `partyList()` returns `[]` before rejoin, `pruneTo` wipes the whole store.
- Impact: The module's stated purpose, persisting read marks so restarts do not mis-report, never takes effect. Every
  rejoin badges the full history in the rail and on every channel.
- Recommendation:
  - Key marks by a stable identity (server fingerprint or normalised address) plus channel id.
  - Do not prune on an empty list; prune only entries whose stable server key was explicitly left.
- Effort: S/M

### UI-06: Whole-conversation refetch on every UI tick, up to 4 Hz during transfers
- Severity: Medium
- Category: performance
- Status: VERIFIED
- Location: `desktop/src/App.jsx:185-218`, `:220-223`; `desktop/src-tauri/src/lib.rs:474-478` (`ui_signature` hashes `t.received`), `:641`; `desktop/src-tauri/src/commands/chats.rs:188-199`; `desktop/src/lib/bridge.js:546-571`
- Problem: Each `state-updated` triggers five IPC calls, including `get_conversation`, which serialises the entire
  `Chat` (every message, with text up to ~24 MiB each) while holding the manager lock. `chatToContact` then re-maps
  every message, calling `fmtTime` and `folderOf` on each. During any file transfer the received byte count changes
  every 250 ms poll, so this happens about 4 times a second for the whole transfer, in every conversation.
  `MSG_WINDOW` only limits rendering. The `visible` `useMemo` in `Messages.jsx:528-538` also never hits, because
  `contact.messages` is a new array on every refresh, despite its comment.
- Impact: CPU, IPC and GC load grow with history size. The manager lock is held while serialising, so this competes
  with network event handling in the poll loop.
- Recommendation:
  - Split transfer progress into its own event, or have the frontend skip `getConversation` when only transfers changed.
  - Add a `messages_since(id, count)` or a "last N" command.
  - Keep a stable messages array when the count and last id are unchanged.
- Effort: M

### UI-07: The invite link is fetched once and cached for the whole session
- Severity: Medium
- Category: bug / ux
- Status: VERIFIED
- Location: `desktop/src/components/Creator.jsx:88-92`
- Problem: The effect fetches `myInviteLink()` only while `!myLink`, and `myLink` is never cleared, including on close
  (see the close-reset effect at `:76-86`). The link carries the address, external (UPnP) address and display name
  (Settings: "Your display name goes into the invite links you share"; "the external address goes into your
  invite"). The UPnP address resolves up to ~15 s after hosting starts (`:94-95`), so a link copied later still has
  the old content. On error the field shows "…" forever with no message.
- Impact: Users share invite links with a stale or missing address or name, and the peer cannot connect.
- Recommendation: Refetch on every open of the invite tab and while hosting (same 3 s tick as `myAddresses`). Show an
  error state.
- Effort: S

### UI-08: Modal focus management overrides every `autoFocus` inside a dialog
- Severity: Medium
- Category: accessibility / ux
- Status: VERIFIED (same pattern reproduced with the repo's React 19 + jsdom: the container ends up focused, not the input)
- Location: `desktop/src/components/ui.jsx:136-149`; affected: `ChatDialogs.jsx:22`, `Creator.jsx:250`, `:267`, `:309`, `Settings.jsx:70`, `PartyAdmin.jsx:221`
- Problem: React applies `autoFocus` during commit, and Modal's passive `useEffect` then runs `ref.current?.focus()`
  on the dialog container. Rename, New connection, Change password and New channel therefore all open with focus on
  the container, not the field the code asks to focus. The keyboard user has to Tab, and in Creator the first stop is
  the mode tabs.
- Impact: Every text dialog needs an extra click or Tab, against the code's intent. The tests assert the container is
  focused (`ui.test.jsx:126`), which is right for confirm dialogs only.
- Recommendation: Add an `initialFocus` prop or ref to Modal, or focus the container only if focus is not already
  inside it (`if (!ref.current.contains(document.activeElement)) ref.current.focus()`). Keep container focus for
  destructive confirmations.
- Effort: S

### UI-09: Toast live regions are `display:none` when empty, so the first announcement may be lost
- Severity: Medium
- Category: accessibility
- Status: SUSPECTED (depends on the screen reader)
- Location: `desktop/src/polish.css:144-145`; `desktop/src/components/Toasts.jsx:27-33`
- Problem: `.toast-region:empty { display:none }` removes both `role=alert` and `role=status` containers from the
  accessibility tree while they are empty. A region that is not rendered when content is inserted is not reliably
  announced (NVDA/JAWS/VoiceOver differ). The comment says toasts are "the app's only channel for errors".
- Impact: Screen-reader users can miss "not delivered", transfer and verification errors.
- Recommendation: Keep the regions rendered and visually empty (`min-height:0`, no padding), or hide them with the
  `.sr-only` technique, not `display:none`.
- Effort: S

### UI-10: `--faint` text fails WCAG AA contrast in every theme
- Severity: Medium
- Category: accessibility
- Status: VERIFIED (computed)
- Location: `desktop/src/app-system.css:17`, `:27`; `themes.css` (`--faint` per theme); used at `app-system.css:119`, `:233`, `:242`, `:285`, `:320`, `:339`, `:392`, `:489`
- Problem: `--faint` is used for small text (10.5–12.5 px): timestamps, hints, empty states, day separators, footer
  notes. Contrast against `--bg`:

  | Theme | Contrast |
  |---|---|
  | dark | 3.51:1 |
  | light | 2.67:1 (3.17:1 on white) |
  | midnight | 3.43:1 |
  | forest | 4.1:1 |
  | rose | 3.92:1 |

  AA needs 4.5:1 for text this size.
- Impact: Hard to read for low-vision users, especially in Daylight.
- Recommendation: Raise `--faint` lightness (dark) or darken it (light) to at least 4.5:1, and add a token-contrast
  test next to `tokens.test.js`.
- Effort: S

### UI-11: Peer-supplied text is not bidi-isolated; linked URLs can display spoofed
- Severity: Medium
- Category: security
- Status: VERIFIED (no handling present; the rendering effect is standard browser behaviour)
- Location: `desktop/src/components/Messages.jsx:15-37` (`URL_RE`, `LinkifiedText`); `app-system.css:330` (`.msg-text`); peer names at `Messages.jsx:125`, `:564`, `Verify.jsx:40`, `:50`, `Creator.jsx:243`, `Contacts.jsx:104`
- Problem: `URL_RE = /(https?:\/\/[^\s<>"']+)/` accepts U+202E/U+2066–2069 and other format characters, and the link
  text equals the href. `https://evil.example/‮moc.knab` therefore renders as a different-looking URL while
  `open_url` opens the real one. Message bubbles, conversation names, Verify titles, mDNS names and contact names
  have no `unicode-bidi: isolate` and no stripping, so an override in one string can also reorder the adjacent
  timestamp or labels. The Rust side strips bidi from filenames and the Party server rejects it in usernames, but P2P
  text and display names reach the DOM unchanged.
- Impact: Phishing links that look legitimate, and spoofed names in the trust UI (for example the "Verify <name>"
  title).
- Recommendation:
  - Add `unicode-bidi: isolate` (or `plaintext`) to `.msg-text`, `.conv-name`, `.chat-head-name`, `.msg-author`,
    `.modal-title` and `.contact-card-name`.
  - In `LinkifiedText`, cut the URL at the first bidi or invisible format character (`\p{Cf}`) or refuse to linkify
    it, and show the punycode host for IDNs.
  - Add a vitest for an RLO URL.
- Effort: S

### UI-12: The file-permission dialog does not reflect the current grant, so Save can revoke rights
- Severity: Medium
- Category: bug
- Status: SUSPECTED
- Location: `desktop/src/components/PartyAdmin.jsx:277-284`, `:319`; `desktop/src-tauri/src/commands/party.rs:237-240`
- Problem: `perms` starts from the **viewer's** effective `file.can_view` / `can_download` (`f.perms`), with `delete`
  and `share` hard-coded to `false`. Switching scope to a member does not load that member's grant either. If the
  default or member grant currently includes share or delete, opening the dialog and pressing Save, perhaps to
  change only "Download it", sends `share:false, delete:false` and silently revokes them.
- Impact: Admins unintentionally strip permissions, with no indication of what the current state was.
- Recommendation: Expose the default and per-member grants in the file DTO, initialise from them, reset on scope
  change, and show "(current)" markers.
- Effort: M

### UI-13: A failed send restores the draft into whichever conversation is now open
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `desktop/src/App.jsx:301-310`
- Problem: `setDraft("")`, then `await api.sendMessage(activeId, text)`, then on failure `setDraft(text)`. If the user
  switched conversations meanwhile, the text appears in the new conversation's composer, and one Enter sends it there.
- Recommendation: Capture `activeId` and restore only if `activeIdRef.current` still equals it; otherwise keep it in
  a per-conversation draft map.
- Effort: S

### UI-14: Relay tokens and host addresses are lost when the dialog closes or the pane changes
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Creator.jsx:76-86` (clears `hostToken`, `hostAddr` on close); `desktop/src/components/Relays.jsx:46`, `:66-69`
- Problem: "Open relay" creates a live rendezvous and shows a token only in local state. Closing the dialog or leaving
  the Relays pane discards it while the session keeps waiting, and there is no way to see the token again. The same
  goes for the host address panel.
- Impact: The user has to open another relay session, leaving the first one orphaned.
- Recommendation: Have the bridge return the active relay token and hosting addresses (e.g. extend `my_addresses`),
  and show them in the hosting placeholder conversation or a titlebar status popover.
- Effort: M

### UI-15: IPv6 addresses are mis-parsed in Connect
- Severity: Low
- Category: bug
- Status: VERIFIED (frontend); backend handling of brackets SUSPECTED
- Location: `desktop/src/components/Creator.jsx:137-146`
- Problem: `lastIndexOf(":")` splits `fe80::1` into host `fe80:` and port `1`. `[::1]:12345` passes host `[::1]`
  with brackets to `connect_peer`; Rust's `(&str,u16)` resolution does not accept bracketed literals.
- Recommendation: Parse `[v6]:port`, treat a host with more than one colon and no brackets as a bare v6 address with
  the default port, and add parse tests in `parse.test.js`.
- Effort: S

### UI-16: Community admin dialogs close and discard input even when the action fails
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Parties.jsx:381-391`, `:413-429`; `PartyAdmin.jsx:201-208`, `:316-322`, `:396-399`
- Problem: `submitAccess`, `shareFile` and `setFilePerms` catch the error and toast it, so their promise resolves and
  the dialogs call `onClose()`. The member selection or permission set the user built is lost.
- Recommendation: Re-throw from the Parties handlers (or return a status) so the dialog stays open, and show the
  error inline.
- Effort: S

### UI-17: "Remove" and "Leave" share one two-click confirmation state
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Parties.jsx:447-482`, `:629-634`, `:648`
- Problem: Clicking banner "Remove" once sets `confirmLeave`. The visible change is on the header Leave button
  ("Leave?"), not on Remove, and Remove has no `onBlur` reset. The user sees nothing happen on Remove, and a later
  single click on Leave leaves immediately. The confirmation text lives only in `title`.
- Recommendation: Use separate state for each button, show the confirm label on the button that was clicked, or use
  a small Modal as Contacts and Chats do.
- Effort: S

### UI-18: Change password does not mention that existing backups keep the old password
- Severity: Low
- Category: security / ux
- Status: VERIFIED
- Location: `desktop/src/components/Settings.jsx:61-69`, `:207-221`; `Onboarding.jsx:218-221` (backup "is already encrypted with the password you just chose")
- Problem: Rotation is described as the fix for "a leaked or shoulder-surfed password", but every identity backup
  exported before the change still opens with the leaked password. Nothing tells the user, and the "Backed up" badge
  stays green.
- Recommendation: After a successful change, mark the backup as needing a refresh: show "Your backups still use the
  old password — export a new one and delete the old copies", and prompt for export.
- Effort: S

### UI-19: Contacts pane gaps (double connect, stale list, inverted state dot)
- Severity: Low
- Category: bug / ux
- Status: VERIFIED
- Location: `desktop/src/components/Contacts.jsx:44-48`, `:18-24`, `:115-116`
- Problem:
  - `connect()` has no busy guard. A double-click calls `connect_contact` twice; Creator's comment says each call
    creates a session.
  - The list loads only on mount or after the user's own actions. A contact promoted to Verified, or one imported
    elsewhere, is not reflected until the pane remounts.
  - A reachable contact gets `state-offline` (grey), and an unreachable one gets `state-connecting` (yellow), which
    reads as "in progress".
- Recommendation: Add a busy flag per row, reload on `state-updated`, and use a neutral dot for unreachable contacts
  instead of the connecting style.
- Effort: S

### UI-20: The thread always jumps to the bottom on any new message
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Messages.jsx:441-443`; `Parties.jsx:290-294`
- Problem: Scroll is forced to the bottom whenever `messageCount` changes, even if the user scrolled up to read
  history (or clicked "Show earlier messages").
- Recommendation: Stick to the bottom only if the user was already within about 80 px of it. Otherwise show a "New
  messages ↓" pill.
- Effort: S

### UI-21: The composer textarea never grows
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Messages.jsx:616`; `Parties.jsx:688`; `app-system.css:422-423`
- Problem: `rows={1}` with no JS autosize and no `field-sizing: content`, so `max-height:140px` is never reached.
  Multi-line drafts (Shift+Enter) show one line at a time.
- Recommendation: Add `field-sizing: content` (Chromium/WebView2) or a small autosize on input.
- Effort: S

### UI-22: Some error paths bypass `friendlyError`
- Severity: Low
- Category: security (minor disclosure) / ux
- Status: VERIFIED
- Location: `desktop/src/App.jsx:139`, `:225`; `Onboarding.jsx:68`; `Parties.jsx:654`
- Problem: Bridge `toast` events, the boot error, and the Party `last_error` banner render raw strings. The
  `friendlyError` docblock explains that these can carry absolute paths with the local username, which then end up
  in screenshots.
- Recommendation: Route them through `friendlyError`, except the fatal identity screen, where the path is useful.
- Effort: S

### UI-23: The chat header labels a fingerprint prefix as an address; Info overstates encryption
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/lib/bridge.js:555`; `Messages.jsx:570`; `ChatDialogs.jsx:64`
- Problem: `chatToContact` sets `address` to `peer_fingerprint.slice(0,16)+"…"`, shown after the state as if it were
  an address. InfoDialog always says "End-to-end encrypted with X25519 + AES-256-GCM", including for `server`
  transport and for chats with no session or no verified fingerprint.
- Recommendation: Rename the field to `fpShort` and label it "Key ab12…". Make the Info text conditional on
  transport and verification.
- Effort: S

### UI-24: Menus and composite widgets lack keyboard semantics
- Severity: Low
- Category: accessibility
- Status: VERIFIED
- Location: `desktop/src/App.jsx:34-62` (ThemeMenu); `Messages.jsx:39-64` (ChatMenu), `:115-121`, `:175` (listbox/option); `Parties.jsx:606-624` (tablist); `Creator.jsx:214-231` (mode tabs)
- Problem:
  - The pop-up menus have no `aria-haspopup` or `aria-expanded`, no Escape to close, no focus move into the menu,
    and no arrow keys.
  - The conversation list declares `role=listbox`/`option` but has no arrow-key navigation (options are separate
    Tab stops).
  - Parties declares `tablist`/`tab` without `aria-controls` or arrow keys.
  - The Creator mode switch exposes neither `aria-pressed` nor tab roles.
  - The theme button's icon is the Settings cog, so two cogs mean different things.
- Recommendation: Implement the WAI-ARIA menu, listbox and tab patterns, or drop the roles and use plain buttons with
  `aria-pressed`/`aria-current`. Give the theme picker a palette icon.
- Effort: M

### UI-25: Icon-only trust badges have no accessible name
- Severity: Low
- Category: accessibility
- Status: VERIFIED
- Location: `desktop/src/components/ui.jsx:61-69`; used at `Messages.jsx:565`, `Contacts.jsx:107`
- Problem: `TrustBadge mini` renders only an SVG, with no text, title or aria-label. In the chat header and on contact
  cards, "Unverified" and "Blocked" are conveyed by colour and icon alone.
- Recommendation: Add `role="img" aria-label={t.label}` and `title={t.label}` when `mini`.
- Effort: S

### UI-26: The Verify dialog can switch peers under the user without notice
- Severity: Low
- Category: security / ux
- Status: VERIFIED
- Location: `desktop/src/App.jsx:205-207`, `:224`; `Verify.jsx:18`, `:40-80`
- Problem: When the head of the prompt queue changes (e.g. the first peer's session dies and is dropped), `fpReq` is
  replaced in place. Title, SAS and target change, but `busy` is kept and there is no visual signal. A user who
  confirmed a code by phone with peer A can then click "Codes match" on peer B's prompt.
- Recommendation: When `req.chat_id` changes, re-mount the dialog (`key={req.chat_id}`), briefly disable the buttons
  (~1 s), and announce "A different connection is now asking to be verified".
- Effort: S

### UI-27: Link handling: auxiliary clicks, silent failures, no confirmation
- Severity: Low
- Category: security / ux
- Status: SUSPECTED (WebView new-window behaviour on middle-click not tested)
- Location: `desktop/src/components/Messages.jsx:26-29`
- Problem: Only `onClick` calls `preventDefault`. Middle-click (`auxclick`) and drag-out are not handled, and the real
  `href` is a peer URL. `openUrl(...).catch(() => {})` swallows refusals, so a click does nothing. Links from
  unverified peers open with no confirmation, which reveals the user's IP to an attacker-chosen host.
- Recommendation:
  - Render as `<button>` or add `onAuxClick`; set `draggable={false}`.
  - Toast on failure.
  - Confirm, showing the full host, for unverified conversations.
- Effort: S

### UI-28: Relays pane shows developer-only instructions and a placeholder section
- Severity: Low
- Category: unfinished / ux
- Status: VERIFIED
- Location: `desktop/src/components/Relays.jsx:18`, `:107-115`
- Problem: "Run your own relay" offers `cargo run --release -p p2pem-classic …` to users of a packaged installer, who
  have neither the source nor cargo. "Saved relays and live routes aren't tracked yet" is a permanent placeholder.
  Relay address input has no Enter handler.
- Recommendation: Link to a release binary or docs page instead of a cargo command. Hide the placeholder section until
  the feature exists.
- Effort: S

### UI-29: Jargon exposed to non-technical users
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Parties.jsx:121-123` ("served by the `messenger-server` crate"); `Onboarding.jsx:160` (a bare 32-hex fingerprint with no label on set-password); `Settings.jsx:241-246` ("UPnP", "mDNS"); `Verify.jsx:42` ("Trust on first use")
- Recommendation: Replace with plain-language copy ("run by the community's operator"), label or remove the
  fingerprint on first run, and keep technical terms in parentheses.
- Effort: S

### UI-30: Incoming file offers show no size; Parties marks threads read while the window is hidden
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Messages.jsx:304-308`; `Parties.jsx:232-236`
- Problem: The Accept/Decline row shows only the filename (the transfer object has `size`), so users accept blindly.
  Separately, `loadServers` marks the on-screen community thread read on every `party-updated` even when the window is
  hidden or unfocused. DMs gate this on `visible` (`App.jsx:238-241`).
- Recommendation: Show `human(t.size)` in the offer. Pass visibility into Parties and skip `markRead` when hidden.
- Effort: S

### UI-31: The global right-click block removes copy from message bubbles
- Severity: Low
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/main.jsx:8-12`
- Problem: The context menu is suppressed for everything except inputs, so users cannot right-click to copy a message,
  link or fingerprint (`.verify-code` even sets `user-select: all`).
- Recommendation: Allow the native menu when `window.getSelection()` is non-empty or the target is inside
  `.msg-text`, code blocks or links.
- Effort: S

### UI-32: Maintainability: dead CSS, duplicated helpers, large components
- Severity: Low
- Category: maintainability
- Status: VERIFIED
- Location: `desktop/src/app-system.css` (893 lines); `Parties.jsx` (725 lines, 17 `useState`); `Creator.jsx:16-38` vs `Relays.jsx:20-41`; `Parties.jsx:167-172` vs `PartyAdmin.jsx:27-33` vs `bridge.js:517-523`
- Problem:
  - Roughly 150 of the 530 CSS classes are never referenced. Examples: `party-pane`, `pd-*`, `pm-*`, `rl-hero*`,
    `rl-route*`, `rl-server*`, `set-log*`, `onb-steps`, `qr-*`, `safety-grid`, `typing-bubble`, `cn-*`. They are
    leftovers of the mockup port; dynamic names such as `btn-*`, `role-*` and `state-*` were excluded from the count.
  - There are three byte formatters with different outputs ("1.0 KB" vs "1 KB", one has no GB) and two identical
    copy-to-clipboard components.
  - `Parties.jsx` mixes data loading, unread bookkeeping, layout and seven admin actions.
  - The `trust === "party"` branches (`Messages.jsx:122`, `:561`) are unreachable: no adapter produces that value.
- Recommendation:
  - Delete the unused CSS.
  - Extract `lib/format.js` (bytes, time) and a single `CopyField`.
  - Split Parties into `useCommunity()` (state and loading), `ServerTabs`, `ChannelSidebar` and `ThreadView`.
- Effort: M

### UI-33: The safety grid is case-sensitive over the hex string
- Severity: Low
- Category: security (latent)
- Status: SUSPECTED (the Rust side appears to emit one case; the mock mixes cases)
- Location: `desktop/src/lib/colorgrid.js:8`, `:14`
- Problem: `charCodeAt` over the raw hex means `A1B2…` and `a1b2…` produce different grids. The mock's own fingerprint
  is uppercase and its peer fingerprints are lowercase. If any path ever formats differently, honest peers see
  mismatching grids.
- Recommendation: Apply `.toLowerCase()` after the filter, and add a test that the grid is case-invariant.
- Effort: S

### UI-34: Set-password explanation path is unreachable
- Severity: Info
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/components/Onboarding.jsx:136-149`, `:177`
- Problem: The submit button is disabled whenever `formError` is set, and implicit submission is blocked when the
  default button is disabled. So `if (formError) { setErr(formError) }` ("Say why nothing happened") never runs.
  With a valid password and an empty confirmation field, nothing explains the disabled button.
- Recommendation: Keep the button enabled and rely on the submit-time message, or render `formError` live under the
  confirmation field.
- Effort: S

### UI-35: i18n readiness
- Severity: Info
- Category: maintainability
- Status: VERIFIED
- Location: all components
- Problem: All strings are hard-coded English, with hand-rolled plurals (`message${n===1?"":"s"}`) and some
  concatenated sentences. Dates do use locale APIs (`toLocale*`), which is good.
- Recommendation: Introduce a message catalogue (even a plain object) and `Intl.PluralRules` before the copy grows
  further.
- Effort: L

### UI-36: No system-theme default
- Severity: Info
- Category: ux
- Status: VERIFIED
- Location: `desktop/src/lib/themes.js:25`
- Problem: The default is always `dark`; `prefers-color-scheme` is ignored. Daylight users get dark on first launch,
  including the lock screen.
- Recommendation: Add an "auto" theme that follows `matchMedia('(prefers-color-scheme: light)')`.
- Effort: S

### UI-37: Test coverage gaps for the bugs above
- Severity: Info
- Category: dx
- Status: VERIFIED
- Location: `desktop/src/*.test.jsx`, `components/*.test.jsx`
- Problem: Every component has a test file, but none covers:
  - out-of-order async responses (UI-02, UI-04)
  - per-thread drafts in Parties (UI-03)
  - the trust=true join carrying the verified fingerprint (UI-01)
  - read marks across a server-id change (UI-05)
  - autofocus inside Modal (UI-08)
  - bidi in linkified text (UI-11)

  Smoke tests cover only first paint, not a flow.
- Recommendation: Add deferred-promise tests (resolve B before A) for App and Parties, and a smoke flow that opens a
  dialog and checks `document.activeElement`.
- Effort: M

## Things done well
- No HTML injection surface: no `dangerouslySetInnerHTML`. Links go through `open_url` with an `isOpenableUrl` mirror
  in the mock. `file_preview` refuses SVG and returns bounded `data:` URLs that are LRU-trimmed
  (`MAX_CACHED_PREVIEWS`).
- The CSP is generated in one place and drift-tested against `tauri.conf.json`. The production `script-src 'self'`
  is asserted by both vitest and Playwright, and dev and built smoke projects are separate.
- The TOFU modal cannot be dismissed but has an escape hatch after an error. SAS leads, with the grid and hex
  demoted. The fail-closed `colorGrid` returns nothing for an invalid fingerprint.
- The risky-executable confirmation is reachable in the browser mock. `friendlyError` strips paths. Strict
  `parsePort` reports what is wrong instead of silently defaulting.
- Unread for DMs is persisted by the bridge. Presence is pushed down with separate focus and visibility gating.
- The `stubApi` built from the real `api` object, plus the `invokeContract` scanner, guard the Tauri arg-name footgun
  on every platform.
- Per-row value memoisation (`convRowsEqual`, `messagesEqual`), a windowed thread (`MSG_WINDOW`), and individually
  imported lucide icons.
- Accessibility basics are present: skip links, a labelled nav landmark, focus-visible outlines, reduced-motion
  handling, a focus trap and focus return in Modal, row status folded into accessible names, and split assertive and
  polite toast regions.
- Creator clears passwords, tokens and addresses on close. ChangePassword clears its fields on close.

## Open questions
- Does `party_download_file` sanitise the peer-supplied `name` it uses as the save-dialog default (`Parties.jsx:355-357`)?
  The UI passes it through unchanged.
- Does `connect_peer` accept bracketed IPv6 literals (UI-15)? If not, the fix belongs on both sides.
- Do the default and per-member file grants already exist server-side in a form the DTO could expose (UI-12)?
- Is the initial `party-updated` emission guaranteed to happen after App registers its listener post-unlock? If it
  is emitted earlier, the Communities rail badge stays 0 until the next change (`App.jsx:228-230`).
- Are P2P display names (`peer_name`, chat titles from peers, mDNS names) filtered for hidden or bidi characters
  anywhere in Rust? UI-11 assumes not.
