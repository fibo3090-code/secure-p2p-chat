// Unified Messages surface — conversation list + chat pane. Clean port of the
// mockup's chat.jsx, driven by live data adapted in lib/bridge.js.
import { memo, useCallback, useMemo, useRef, useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { api, fmtDay, friendlyError } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";
import { cx, Avatar, IconButton, Button, Modal, arrowNav } from "./ui.jsx";
import { fileKind, splitName } from "../lib/fileKind.js";
import { Logo } from "./Logo.jsx";

const IMAGE_RE = /\.(png|jpe?g|gif|webp|bmp)$/i;
const isImageName = (name) => IMAGE_RE.test(name || "");

// Render message text with http(s) URLs clickable. Opening goes through the
// bridge's open_url (scheme re-checked there) so links launch the system
// browser, never navigate the webview. Trailing punctuation stays text.
const URL_RE = /(https?:\/\/[^\s<>"']+)/gi;
function LinkifiedText({ text }) {
  const parts = String(text ?? "").split(URL_RE);
  if (parts.length === 1) return <>{text}</>;
  return (
    <>
      {parts.map((part, i) => {
        if (i % 2 === 0) return part;
        const trimmed = part.replace(/[.,;:!?)\]]+$/, "");
        const rest = part.slice(trimmed.length);
        return (
          <span key={i}>
            <a className="msg-link" href={trimmed} title={trimmed}
              onClick={(e) => { e.preventDefault(); api.openUrl(trimmed).catch(() => {}); }}>
              {trimmed}
            </a>
            {rest}
          </span>
        );
      })}
    </>
  );
}

function ChatMenu({ contact, onVerify, onRename, onDelete, onInfo }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    const onKey = (e) => { if (e.key === "Escape") setOpen(false); };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("mousedown", onDown); window.removeEventListener("keydown", onKey); };
  }, [open]);
  const item = (fn) => () => { setOpen(false); fn && fn(contact); };
  // A listener has no peer yet, so there is nothing to verify and nothing to
  // rename it after: offering either was a control that could only fail.
  const isPlaceholder = contact.placeholder;
  return (
    <div className="chat-head-actions">
      <IconButton name="info" label="Conversation info" onClick={() => onInfo && onInfo(contact)} />
      <div className="pop-wrap" ref={ref}>
        <IconButton name="more" label="More actions" active={open} aria-haspopup="menu" aria-expanded={open}
          onClick={() => setOpen((o) => !o)} />
        {open && (
          <div className="pop-menu" role="menu" onKeyDown={arrowNav('[role="menuitem"]')}
            ref={(el) => { if (el && !el.contains(document.activeElement)) el.querySelector('[role="menuitem"]')?.focus(); }}>
            {!isPlaceholder && (
              <button role="menuitem" onClick={item(onVerify)}><Icon name="fingerprint" size={15} /> Verify safety code</button>
            )}
            {!isPlaceholder && (
              <button role="menuitem" onClick={item(onRename)}><Icon name="edit" size={15} /> Rename</button>
            )}
            <div className="pop-sep" role="separator" />
            <button role="menuitem" className="is-danger" onClick={item(onDelete)}>
              <Icon name="trash" size={15} /> {isPlaceholder ? "Stop listening" : "Delete conversation"}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

const STATE_LABEL = { connected: "Connected", hosting: "Listening for a peer", offline: "Offline", connecting: "Connecting…" };
const TRANSPORT_WORD = { direct: "Direct", relay: "Via relay", server: "Via server" };

// What a listener shows instead of an empty, black thread: the address a peer
// needs, ready to copy. "Share your address" with no address on screen was a
// dead end.
function HostingCard({ onStop, paused }) {
  const [addr, setAddr] = useState(null);
  const [invite, setInvite] = useState("");
  useEffect(() => {
    let live = true;
    api.myAddresses().then((a) => live && setAddr(a)).catch(() => {});
    api.myInviteLink().then((l) => live && setInvite(l || "")).catch(() => {});
    return () => { live = false; };
  }, []);
  const copy = (text, what) => navigator.clipboard?.writeText(text)
    .then(() => toast(`${what} copied`, "success"))
    .catch(() => toast("Could not copy", "error"));
  const lines = [
    addr?.local && { label: "On this network", value: addr.local },
    addr?.external && { label: "Over the internet", value: addr.external },
  ].filter(Boolean);
  return (
    <div className={cx("hosting-card", paused && "is-paused")}>
      <span className="hosting-pulse" aria-hidden="true"><Icon name={paused ? "ban" : "radio"} size={24} /></span>
      <div className="hosting-h">{paused ? "Paused — not accepting new peers" : "Waiting for someone to connect"}</div>
      <p className="hosting-p">
        {paused
          ? "New peers are refused while “Accepting” is switched off in the sidebar. Switch it back on to let someone connect."
          : "Give the other person one of these. When they connect you'll both see a safety code to compare before anything is sent."}
      </p>
      {lines.length > 0 ? (
        <div className="hosting-addrs">
          {lines.map((l) => (
            <div key={l.label} className="hosting-addr">
              <span className="hosting-addr-l">{l.label}</span>
              <code className="hosting-addr-v">{l.value}</code>
              <button className="hosting-copy" onClick={() => copy(l.value, "Address")} aria-label={`Copy ${l.label.toLowerCase()} address`}>
                <Icon name="copy" size={14} /> Copy
              </button>
            </div>
          ))}
        </div>
      ) : (
        <div className="hosting-p">Looking up your address…</div>
      )}
      <div className="hosting-actions">
        {invite && (
          <Button size="sm" variant="ghost" icon="link" onClick={() => copy(invite, "Invite link")}>Copy invite link</Button>
        )}
        {onStop && <Button size="sm" variant="danger-ghost" icon="x" onClick={onStop}>Stop listening</Button>}
      </div>
    </div>
  );
}

// Shown above a disabled composer. Messages only exist once a session carries
// them, so say which situation this is and what would fix it — the alternative
// (accept the text and drop it) is how a typed paragraph disappears.
const OFFLINE_NOTICE = {
  offline: "Not connected — messages can't be sent yet. Reconnect to this peer, or ask them to connect to you.",
  hosting: "Waiting for a peer to connect. Share your address, then you can start the conversation.",
  connecting: "Connecting… you'll be able to send as soon as the peer answers.",
};

// The markers beside a conversation are icons and a bare count, so a screen
// reader reached the row and announced only the name and preview — the
// unverified warning and the unread badge, the two things that decide whether
// you open it, were invisible. Spelled out here and stitched into the row's
// accessible name.
export function rowStatusLabel(c) {
  const bits = [];
  if (c.unread > 0) bits.push(`${c.unread} unread message${c.unread === 1 ? "" : "s"}`);
  if (c.kind === "group") bits.push("group");
  if (c.kind === "channel") bits.push("channel");
  if (c.transport === "relay") bits.push("over a relay");
  if (c.transport === "server") bits.push("over a community server");
  if (c.state === "hosting") bits.push("hosting, waiting for a peer");
  else if (c.state === "connected") bits.push("connected");
  else if (c.state === "connecting") bits.push("connecting");
  else bits.push("offline");
  if (c.trust === "verified" && c.state !== "hosting") bits.push("verified");
  if (c.trust === "unverified" && c.state !== "hosting") bits.push("not verified");
  return bits.join(", ");
}

// Memoised by value, not by identity: the conversation list is rebuilt from a
// fresh bridge payload on every poll tick, so every row object is new each time
// and the default shallow compare would never hit. Comparing the fields the row
// actually renders is what stops an idle poll from re-rendering the whole list.
function convRowsEqual(a, b) {
  if (a.onSelect !== b.onSelect || a.active !== b.active || a.paused !== b.paused || a.tabStop !== b.tabStop) return false;
  const x = a.c;
  const y = b.c;
  return (
    x.id === y.id && x.name === y.name && x.last === y.last && x.lastT === y.lastT &&
    x.typing === y.typing && x.kind === y.kind && x.transport === y.transport &&
    x.state === y.state && x.trust === y.trust && x.unread === y.unread &&
    x.lastFromMe === y.lastFromMe
  );
}

const ConvRow = memo(function ConvRow({ c, active, onSelect, paused, tabStop = true }) {
  const status = rowStatusLabel(c);
  const hosting = c.state === "hosting";
  return (
    <button
      className={cx("conv-row", active && "is-active", c.unread > 0 && "has-unread")}
      role="option"
      aria-selected={active}
      tabIndex={tabStop ? 0 : -1}
      aria-label={`${c.name}. ${status}`}
      onClick={() => onSelect(c.id)}
    >
      <Avatar name={c.name} size={44} state={hosting ? undefined : c.state}
        party={c.trust === "party"} icon={hosting ? "radio" : undefined} />
      <div className="conv-main">
        <div className="conv-line1">
          <span className="conv-name">{c.name}</span>
          {/* aria-hidden: every marker here is repeated in the row's aria-label
              above, in words. Leaving them exposed makes a reader announce a
              bare glyph after the name. */}
          <span className="conv-time">{c.lastT}</span>
        </div>
        <div className="conv-line2">
          <span className="conv-preview">
            {c.typing ? <em className="conv-typing">typing…</em>
              : <>{c.lastFromMe && c.last && <span className="conv-you">You: </span>}{c.last}</>}
          </span>
          <span className="conv-markers" aria-hidden="true">
            {c.kind === "group" && <Icon name="users" size={12} />}
            {c.kind === "channel" && <Icon name="hash" size={12} />}
            {hosting && (paused
              ? <span className="conv-chip is-paused">Paused</span>
              : <span className="conv-chip is-hosting">Listening</span>)}
            {c.transport === "relay" && <span className="conv-chip" title="Via relay">Relay</span>}
            {c.transport === "server" && <span className="conv-chip" title="Via server">Server</span>}
            {/* Only a real peer can be unverified. A listener with nobody on
                the other end used to wear the same warning, and a warning that
                shows up where nothing is wrong is one people learn to skip. */}
            {c.unread > 0 && <span className="conv-unread">{c.unread > 99 ? "99+" : c.unread}</span>}
          </span>
        </div>
      </div>
    </button>
  );
}, convRowsEqual);

export function ConvList({ contacts, activeId, onSelect, onAdd, query, setQuery, connectedCount = 0, gateClosed = false, status }) {
  const filtered = useMemo(
    () => contacts.filter((c) => c.name.toLowerCase().includes(query.toLowerCase())),
    [contacts, query],
  );
  // Total unread, announced politely. A badge appearing on a row the user is
  // not looking at is otherwise a purely visual event.
  const totalUnread = useMemo(
    () => filtered.reduce((n, c) => n + (c.unread || 0), 0),
    [filtered],
  );
  return (
    <div className="conv-list">
      <div className="conv-head">
        <div className="conv-head-txt">
          <h1 className="conv-title">Chats</h1>
          {status || (
            <span className="conv-sub">
              {connectedCount > 0
                ? <><span className="conv-sub-dot" aria-hidden="true" />{connectedCount} connected</>
                : "Nobody connected"}
            </span>
          )}
        </div>
        <button className="conv-add" onClick={onAdd} title="New connection" aria-label="New connection">
          <Icon name="compose" size={17} />
        </button>
      </div>
      <label className="conv-search">
        <Icon name="search" size={15} />
        <input placeholder="Search" aria-label="Search conversations"
          value={query} onChange={(e) => setQuery(e.target.value)} />
      </label>
      <div className="sr-only" role="status" aria-live="polite">
        {totalUnread > 0 ? `${totalUnread} unread message${totalUnread === 1 ? "" : "s"}` : "No unread messages"}
      </div>
      {/* listbox/option: the rows are a single-select list of conversations,
          not a run of unrelated buttons, so a reader announces "3 of 12"
          instead of leaving the user to count. */}
      <div className="conv-scroll" role="listbox" aria-label="Conversations" onKeyDown={arrowNav('[role="option"]')}>
        {filtered.map((c, i) => (
          <ConvRow key={c.id} c={c} active={activeId === c.id} onSelect={onSelect} paused={gateClosed}
            tabStop={activeId ? activeId === c.id : i === 0} />
        ))}
        {filtered.length === 0 && (
          <div className="conv-empty">
            {contacts.length === 0 ? (
              <>
                No conversations yet.
                <button className="conv-empty-cta" onClick={onAdd}>
                  <Icon name="plus" size={14} /> Start a connection
                </button>
              </>
            ) : (
              "No conversations match your search."
            )}
          </div>
        )}
      </div>
    </div>
  );
}

// A sent message's delivery state, as an icon with its word attached. One
// tick: sent from this device. Two: the peer's app confirmed receipt.
function Receipt({ delivered }) {
  return delivered
    ? <span className="msg-receipt is-delivered" title="Delivered"><Icon name="checkCheck" size={14} /><span className="sr-only">Delivered</span></span>
    : <span className="msg-receipt" title="Sent"><Icon name="check" size={14} /><span className="sr-only">Sent</span></span>;
}

function MessageItemInner({ m, preview, onOpen, onReveal, pos = "single", peerName }) {
  if (m.kind === "system") {
    return (
      <div className={cx("sys-msg", m.warn && "is-warn", m.ok && "is-ok")}>
        <Icon name={m.warn ? "alert" : m.ok ? "shieldCheck" : "lock"} size={13} />
        <span>{m.text}</span>
        <span className="sys-t">{m.t}</span>
      </div>
    );
  }
  const mine = m.from === "me";
  if (m.kind === "file") {
    const done = m.progress >= 100;
    const kind = fileKind(m.name);
    // A received file the OS would execute says so on the card, before anyone
    // clicks it. The refusal itself stays in the bridge — this is the warning
    // label on the box, not the lock on it.
    const risky = kind.risky && !mine;
    const { stem, ext } = splitName(m.name);
    // Name the folder the file actually went to. Hardcoding "Downloads" was
    // wrong for anyone who picked a different download folder — and this card
    // is the only place the app ever says where a received file landed.
    const where = done
      ? (mine ? "Sent" : m.dir ? `Saved to ${m.dir}` : "Saved")
      : `${m.progress}%`;
    const canOpen = done && m.hasPath;
    const open = canOpen && onOpen ? () => onOpen(m) : undefined;
    return (
      <div className={cx("msg-row", mine ? "is-mine" : "is-them", "pos-" + pos)}>
        <div className={cx("file-card", mine && "is-mine", canOpen && "is-openable", risky && "is-risky")}>
          {preview && (
            <button className="file-thumb" onClick={open} title={"Open " + m.name}>
              <img src={preview} alt={m.name} loading="lazy" />
            </button>
          )}
          <span className="sr-only">{mine ? "You sent a file: " : `${peerName || "They"} sent a file: `}</span>
          <div className="file-card-row">
            <button className="file-ic" onClick={open} disabled={!canOpen}
              aria-label={canOpen ? `Open ${m.name}` : m.name}
              title={canOpen ? "Open file" : undefined}><Icon name={kind.icon} size={20} /></button>
            <button className="file-meta" onClick={open} disabled={!canOpen}
              title={m.path || (canOpen ? "Open file" : undefined)}>
              <div className="file-name">
                {/* The final extension is the one the OS acts on, so it is the
                    one that must not be lost to an ellipsis: a long stem
                    truncates, ".exe" never does. */}
                <span className="file-stem">{ext ? stem : m.name}</span>
                {ext && <span className={cx("file-ext", risky && "is-risky")}>.{ext}</span>}
              </div>
              <div className="file-sub">
                {risky
                  ? <span className="file-risk"><Icon name="alert" size={12} /> Program — runs code if opened</span>
                  : <span>{kind.label} · {m.size}</span>}
              </div>
            </button>
            {canOpen && (
              <button className="file-reveal" title="Show in folder" aria-label={`Show ${m.name} in folder`}
                onClick={() => onReveal && onReveal(m)}><Icon name="folder" size={16} /></button>
            )}
          </div>
          <div className="file-foot">
            <span className="file-where" title={m.path || undefined}>
              {risky ? <>{m.size} · {where}</> : where}
            </span>
            <span className="msg-meta">
              {m.t}
              {mine && done && <Receipt delivered={m.delivered} />}
            </span>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className={cx("msg-row", mine ? "is-mine" : "is-them", "pos-" + pos)}>
      <div className="msg-bubble">
        {m.author && !mine && <span className="msg-author">{m.author}</span>}
        {/* Left/right is a visual cue only; say who wrote it. */}
        <span className="sr-only">{mine ? "You: " : `${m.author || peerName || "Them"}: `}</span>
        <span className="msg-text"><LinkifiedText text={m.text} /></span>
        {/* The spacer reserves room for the timestamp on the last line, so
            short messages keep it inline and long ones never run under it. */}
        <span className="msg-meta-spacer" aria-hidden="true" />
        <span className="msg-meta">
          <time dateTime={m.ts || undefined}>{m.t}</time>
          {mine && <Receipt delivered={m.delivered} />}
        </span>
      </div>
    </div>
  );
}

// Memoised by value. `chatToContact` rebuilds the whole message array from a
// fresh bridge payload on every poll tick, so each `m` is a new object and the
// default shallow compare never hits — a 150-message thread re-rendered every
// row, several times a second, for the entire time the app was open. Comparing
// the fields a row actually renders makes an idle tick cost nothing.
function messagesEqual(a, b) {
  if (a.preview !== b.preview || a.onOpen !== b.onOpen || a.onReveal !== b.onReveal || a.pos !== b.pos ||
      a.peerName !== b.peerName) return false;
  const x = a.m;
  const y = b.m;
  return (
    x.id === y.id && x.kind === y.kind && x.text === y.text && x.t === y.t &&
    x.from === y.from && x.author === y.author && x.delivered === y.delivered &&
    x.name === y.name && x.size === y.size && x.progress === y.progress &&
    x.hasPath === y.hasPath && x.path === y.path && x.dir === y.dir &&
    x.warn === y.warn && x.ok === y.ok && x.ts === y.ts
  );
}

const MessageItem = memo(MessageItemInner, messagesEqual);

function transferPct(t) {
  return t.size > 0 ? Math.min(100, Math.round((t.received / t.size) * 100)) : 0;
}

function transferLabel(t) {
  if (t.status === "failed") return t.error || "failed";
  if (t.status === "cancelled") return "cancelled";
  if (t.status === "done") return "done";
  if (t.status === "awaiting") return "incoming file";
  return `${transferPct(t)}%`;
}

// Live progress cards for the conversation's in-flight file transfers, so a
// large send/receive shows movement instead of nothing until completion.
// An "awaiting" transfer is an incoming offer: it shows Accept / Decline
// instead of a progress bar (nothing is saved until the user accepts).
// In-flight transfers get a cancel button (either direction).
function TransferBar({ transfers, onAccept, onDecline, onCancel }) {
  if (!transfers || transfers.length === 0) return null;
  return (
    <div className="transfer-bar">
      {transfers.map((t) => (
        <div key={t.id} className={cx("transfer-item", "is-" + t.status)}>
          <Icon name={t.direction === "outgoing" ? "arrowUp" : "arrowDown"} size={13} />
          <span className="transfer-name">{t.filename}</span>
          {t.status === "awaiting" && t.direction !== "outgoing" ? (
            <span className="transfer-offer">
              <Button icon="check" onClick={() => onAccept && onAccept(t)}>Accept</Button>
              <Button variant="danger-ghost" icon="x" onClick={() => onDecline && onDecline(t)}>Decline</Button>
            </span>
          ) : (
            <div className="transfer-track"><div className="transfer-fill" style={{ width: `${transferPct(t)}%` }} /></div>
          )}
          <span className="transfer-pct mono">{transferLabel(t)}</span>
          {t.cancellable && (
            <button
              className="transfer-cancel"
              title="Cancel transfer"
              aria-label={`Cancel transfer of ${t.filename}`}
              onClick={() => onCancel && onCancel(t.id)}
            >
              <Icon name="x" size={12} />
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

// The three real ways to reach someone for the first time. There is no account
// system and no directory, so a user arriving from a mainstream messenger has
// no working mental model — spelling the options out here is the difference
// between "this app is broken" and a first successful connection.
const START_PATHS = [
  {
    mode: "invite",
    icon: "user",
    title: "Send them an invite link",
    body: "Copy your link and send it however you already talk — chat, email, a message. They paste it in and connect.",
    hint: "Easiest if you can reach them somewhere else already",
  },
  {
    mode: "connect",
    icon: "send",
    title: "Dial their address",
    body: "If they are already hosting, enter the address they gave you (like 192.168.1.40:12345) and connect.",
    hint: "Works on the same network with no setup",
  },
  {
    mode: "host",
    icon: "server",
    title: "Let them come to you",
    body: "Start listening and share the address the app shows you. They dial it from their side.",
    hint: "Across the internet this needs a port forward or a relay",
  },
];

function GetStarted({ onStart }) {
  return (
    <div className="chat-pane chat-start">
      <div className="chat-start-inner">
        <div className="chat-start-head">
          <span className="chat-empty-ic"><Icon name="shieldCheck" size={26} /></span>
          <div>
            <div className="chat-start-h">Let’s get you connected</div>
            <div className="chat-start-p">
              There are no accounts here and no directory to search — your messages
              go straight to the other person. That means the first connection
              takes one deliberate step. Pick whichever is easiest:
            </div>
          </div>
        </div>

        <div className="chat-start-paths">
          {START_PATHS.map((p) => (
            <button key={p.mode} className="start-card" onClick={() => onStart(p.mode)}>
              <span className="start-card-ic"><Icon name={p.icon} size={18} /></span>
              <span className="start-card-txt">
                <span className="start-card-h">{p.title}</span>
                <span className="start-card-b">{p.body}</span>
                <span className="start-card-hint">{p.hint}</span>
              </span>
              <span className="start-card-go"><Icon name="chevronRight" size={16} /></span>
            </button>
          ))}
        </div>

        <div className="chat-start-foot">
          <Icon name="fingerprint" size={14} />
          <span>
            When you connect, both of you will see the same six digits and three
            emoji. Read them out over a call — if they match, nobody is in the
            middle. That check is what makes the encryption mean something.
          </span>
        </div>
      </div>
    </div>
  );
}

// How many messages render at once. Long histories only mount their tail, so a
// 10k-message thread doesn't re-render thousands of nodes on every poll tick;
// "Show earlier messages" widens the window on demand.
const MSG_WINDOW = 150;

// Messages closer together than this, from the same side, are one "turn".
const GROUP_GAP_MS = 5 * 60 * 1000;

// How many image previews are held as `data:` URLs at once. Each is a
// base64-encoded copy of the file in webview memory (up to the bridge's 4 MiB
// cap), so an image-heavy thread could otherwise pin hundreds of megabytes just
// by being scrolled through. Keeping the most recent handful covers what is on
// screen; anything older is re-fetched if the user scrolls back.
const MAX_CACHED_PREVIEWS = 24;

/// Keep only the newest `MAX_CACHED_PREVIEWS` entries, preserving insertion
/// order (JS objects keep string-key insertion order for non-numeric keys, and
/// message ids are UUIDs).
function trimPreviews(map) {
  const keys = Object.keys(map);
  if (keys.length <= MAX_CACHED_PREVIEWS) return map;
  const keep = keys.slice(-MAX_CACHED_PREVIEWS);
  return Object.fromEntries(keep.map((k) => [k, map[k]]));
}

export function ChatPane({ contact, onVerify, onRename, onDelete, onInfo, onSendFile, draft, setDraft, onSend, transfers, onAcceptTransfer, onDeclineTransfer, onCancelTransfer, isFirstRun, onStart, reconnectContact, onReconnect, gateClosed, waiting, onJump }) {
  const scrollRef = useRef(null);
  const [shown, setShown] = useState(MSG_WINDOW);
  // Inline previews for image files, fetched once per message id (null =
  // asked, not previewable) so the poll-driven re-renders never refetch.
  const [previews, setPreviews] = useState({});
  // A peer-sent file the OS would execute, held for the user's confirmation.
  const [riskyOpen, setRiskyOpen] = useState(null);
  const requested = useRef(new Set());
  const contactId = contact ? contact.id : null;
  const messageCount = contact ? contact.messages.length : 0;
  // The conversation on screen *now*, for async work started on an earlier
  // render to check before it writes (see the preview effect below).
  const visibleContact = useRef(contactId);
  useEffect(() => {
    visibleContact.current = contactId;
    setShown(MSG_WINDOW);
    setPreviews({});
    requested.current = new Set();
  }, [contactId]);
  // Follow the thread only when the reader is already at the bottom. Yanking
  // someone reading history down to every new arrival loses their place; a
  // pill says something arrived instead.
  const atBottom = useRef(true);
  const [newBelow, setNewBelow] = useState(false);
  const toBottom = () => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    setNewBelow(false);
  };
  useEffect(() => { atBottom.current = true; toBottom(); }, [contactId]);
  useEffect(() => {
    if (atBottom.current) toBottom(); else setNewBelow(true);
  }, [messageCount]);
  const onThreadScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (atBottom.current) setNewBelow(false);
  };
  // Which file cards are currently previewable. Keying the effect on this —
  // rather than on the message count — means a card that becomes eligible
  // without the thread growing (it gains a path, or finishes) still gets its
  // thumbnail. `requested` keeps repeat evaluations idempotent.
  const previewKey = (contact ? contact.messages.slice(-shown) : [])
    .filter((m) => m.kind === "file")
    .map((m) => `${m.id}:${m.hasPath ? 1 : 0}:${m.progress}`)
    .join(",");
  useEffect(() => {
    if (!contact) return;
    const want = contact.messages
      .slice(-shown)
      .filter((m) => m.kind === "file" && m.hasPath && m.progress >= 100 &&
                     isImageName(m.name) && !requested.current.has(m.id));
    if (want.length === 0) return;
    want.forEach((m) => requested.current.add(m.id));
    const forContact = contact.id;
    (async () => {
      const updates = {};
      for (const m of want) {
        try { updates[m.id] = await api.filePreview(forContact, m.id); }
        catch { updates[m.id] = null; }
      }
      // Dropped only if the user has since switched conversations. This used
      // to be an effect-cleanup flag, so *any* re-run cancelled the write —
      // and the effect re-runs whenever another file card's progress ticks.
      // The ids were already in `requested`, so a thumbnail fetched during
      // someone else's transfer was thrown away and never asked for again.
      //
      // Bounded: `requested` still remembers what was asked for, so an evicted
      // preview is not re-fetched in a loop — it simply renders as a plain card
      // until the conversation is reopened.
      if (visibleContact.current === forContact) {
        setPreviews((p) => trimPreviews({ ...p, ...updates }));
      }
    })();
  }, [contactId, previewKey, shown]); // eslint-disable-line react-hooks/exhaustive-deps -- `contact` changes identity every poll; previewKey captures what matters

  // Opening a peer's file can mean running their code. The bridge refuses
  // those unless confirmed and tells us what it is, so the click turns into a
  // question instead of an execution.
  // useCallback, because these are the props `MessageItem`'s memo compares by
  // identity — recreating them each render would defeat it entirely.
  const openFile = useCallback(async (m) => {
    try {
      const r = await api.openFile(contactId, m.id, false);
      if (r && r.blocked) setRiskyOpen({ msg: m, kind: r.blocked, filename: r.filename || m.name });
    } catch (e) { toast(friendlyError(e), "error"); }
  }, [contactId]);
  const confirmRiskyOpen = async () => {
    const target = riskyOpen;
    setRiskyOpen(null);
    if (!target) return;
    try { await api.openFile(contact.id, target.msg.id, false, true); }
    catch (e) { toast(friendlyError(e), "error"); }
  };
  // Revealing never launches anything, so it is never gated.
  const revealFile = useCallback((m) =>
    api.openFile(contactId, m.id, true)
      .catch((e) => toast(`Could not show the file: ${friendlyError(e)}`, "error")),
  [contactId]);

  // Sending needs a live session. Without this the composer accepted a whole
  // paragraph, the bridge reported success, and the text vanished.
  const online = contact ? contact.state === "connected" : false;
  const hosting = contact ? contact.state === "hosting" : false;
  const canRedial = !!(reconnectContact && reconnectContact.reachable && reconnectContact.trust !== "blocked");
  const [redial, setRedial] = useState({ busy: false, failed: "" });
  useEffect(() => { setRedial({ busy: false, failed: "" }); }, [contactId]);
  const doReconnect = async () => {
    setRedial({ busy: true, failed: "" });
    try { await onReconnect(contact); setRedial({ busy: false, failed: "" }); }
    catch (e) { setRedial({ busy: false, failed: friendlyError(e) }); }
  };
  const copyInvite = () => api.myInviteLink()
    .then((l) => navigator.clipboard?.writeText(l))
    .then(() => toast("Invite link copied", "success"))
    .catch(() => toast("Could not copy the invite link", "error"));
  // One outgoing file at a time per conversation: `FileChunk` carries no
  // transfer id, so two concurrent sends interleave on the wire and corrupt
  // both files. The backend refuses the second send; the button must not
  // pretend otherwise.
  const sending = (transfers || []).some(
    (t) => t.direction === "outgoing" && (t.status === "pending" || t.status === "active"),
  );
  const clipTitle = !online
    ? "Not connected"
    : sending
      ? "Already sending a file in this conversation"
      : "Send file";

  // The rendered window and its day separators, recomputed only when the thread
  // or the window actually changes. `Date` parsing per message on every poll
  // tick was the other half of the re-render cost.
  //
  // Declared before the early returns below: hooks must run in the same order on
  // every render, so this cannot move inside the JSX.
  const messages = contact ? contact.messages : null;
  const visible = useMemo(() => {
    if (!messages) return [];
    let lastDay = null;
    const rows = messages.slice(-shown).map((m, i) => {
      const day = m.ts ? new Date(m.ts).toDateString() : null;
      const sep = day && day !== lastDay ? fmtDay(m.ts) : null;
      if (day) lastDay = day;
      return { m, sep, key: m.id || `i${i}` };
    });
    // Group runs: consecutive messages from the same side, on the same day,
    // within a few minutes, read as one turn — tight spacing, and only the last
    // bubble of the run gets the "tail" corner. Every message keeps its own
    // timestamp; grouping changes the rhythm, never the information.
    const joins = (a, b) => a && b && !b.sep && a.m.kind !== "system" && b.m.kind !== "system" &&
      a.m.from === b.m.from && a.m.ts && b.m.ts &&
      Math.abs(new Date(b.m.ts) - new Date(a.m.ts)) < GROUP_GAP_MS;
    return rows.map((r, i) => {
      const prev = joins(rows[i - 1], r);
      const next = joins(r, rows[i + 1]);
      return { ...r, pos: prev && next ? "mid" : prev ? "last" : next ? "first" : "single" };
    });
  }, [messages, shown]);

  if (!contact) {
    // With nothing to select, "Select a conversation" is a dead end — and this
    // is exactly the moment a new user decides whether the app works. There are
    // no accounts and no directory here, so the first connection genuinely needs
    // explaining; show the three real paths instead of a shrug.
    if (isFirstRun) return <GetStarted onStart={onStart} />;
    return (
      <div className="chat-pane chat-empty">
        <div className="chat-empty-inner">
          <span className="chat-empty-logo"><Logo size={64} /></span>
          {waiting && waiting.total > 0 ? (
            <>
              <div className="chat-empty-h">
                {waiting.total} unread {waiting.total === 1 ? "message" : "messages"}
                {waiting.chats > 1 ? ` in ${waiting.chats} conversations` : ""}
              </div>
              <div className="chat-empty-p">
                <button className="chat-empty-link" onClick={() => onJump && onJump(waiting.firstId)}>
                  Open {waiting.firstName}
                </button>
                {" "}or pick a conversation on the left.
              </div>
            </>
          ) : (
            <>
              <div className="chat-empty-h">Select a conversation</div>
              <div className="chat-empty-p">Pick a conversation on the left, or start a new connection.</div>
            </>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="chat-pane">
      <header className="chat-head">
        <div className="chat-head-l">
          <Avatar name={contact.name} size={38} state={hosting ? undefined : contact.state}
            party={contact.trust === "party"} icon={hosting ? "radio" : undefined} />
          <div className="chat-head-info">
            <h2 className="chat-head-name">{contact.name}</h2>
            {/* Plain words, not a row of glyphs and a hex string: whether the
                peer is here, how the bytes travel, and whether you checked it
                is really them. The trust word is also the way to act on it. */}
            <div className="chat-head-sub">
              <span className={"chat-state state-txt-" + contact.state}>{STATE_LABEL[contact.state]}</span>
              {!hosting && (
                <>
                  {contact.transport && contact.transport !== "direct" && (
                    <>
                      <span className="chat-dot" aria-hidden="true">·</span>
                      <span>{TRANSPORT_WORD[contact.transport]}</span>
                    </>
                  )}
                  <span className="chat-dot" aria-hidden="true">·</span>
                  <button className={cx("chat-trust", contact.trust === "verified" ? "is-ok" : "is-pending")}
                    onClick={() => onVerify && onVerify(contact)}
                    title={contact.trust === "verified" ? "Show fingerprints" : "How to verify this contact"}>
                    <Icon name={contact.trust === "verified" ? "shieldCheck" : "shieldAlert"} size={13} />
                    {contact.trust === "verified" ? "Verified" : "Not verified"}
                  </button>
                </>
              )}
            </div>
          </div>
        </div>
        <ChatMenu contact={contact} onVerify={onVerify} onRename={onRename} onDelete={onDelete} onInfo={onInfo} />
      </header>

      {/* A log: a screen reader announces arrivals in the open conversation,
          and the region is focusable so the keyboard can scroll history. */}
      <div className="chat-scroll" ref={scrollRef} role="log" aria-live="polite"
        aria-label={`Messages with ${contact.name}`} tabIndex={0} onScroll={onThreadScroll}>
        {hosting && <HostingCard paused={gateClosed} onStop={onDelete ? () => onDelete(contact) : null} />}
        <div className="chat-thread" hidden={hosting && contact.messages.length === 0}>
          {!hosting && (
            // Said once, quietly, where the conversation starts: what protects
            // it and whether it has been checked. Nothing else in the thread
            // ever told the user their messages were encrypted.
            <button className={cx("e2e-note", contact.trust === "verified" ? "is-ok" : "is-warn")}
              onClick={() => onVerify && onVerify(contact)}>
              <Icon name="lock" size={13} />
              <span>
                End-to-end encrypted{contact.transport === "relay" ? " — the relay can't read messages, but sees both IP addresses" : ""}.{" "}
                {contact.trust === "verified"
                  ? <span className="e2e-ok">Safety code verified.</span>
                  : online
                    ? <span className="e2e-action">Compare safety codes.</span>
                    : <span className="e2e-later">You'll verify on the next connection.</span>}
              </span>
            </button>
          )}
          {contact.messages.length > shown && (
            <button className="thread-more" onClick={() => setShown((s) => s + MSG_WINDOW)}>
              Show earlier messages ({contact.messages.length - shown} more)
            </button>
          )}
          {visible.map(({ m, sep, key, pos }) => (
            <div key={key}>
              {/* A date break is a structural divider, not decoration: without
                  the role a screen reader reads the date as a stray line of
                  text between two messages, with nothing saying the thread has
                  moved to another day. */}
              {sep && (
                <div className="day-sep" role="separator" aria-label={`Messages from ${sep}`}>
                  <span>{sep}</span>
                </div>
              )}
              <MessageItem m={m} preview={m.id ? previews[m.id] : null} pos={pos} peerName={contact.name}
                onOpen={openFile} onReveal={revealFile} />
            </div>
          ))}
        </div>
      </div>

      {newBelow && (
        <button className="new-below" onClick={toBottom}>
          <Icon name="arrowDown" size={14} /> New messages
        </button>
      )}

      <TransferBar transfers={transfers} onAccept={onAcceptTransfer} onDecline={onDeclineTransfer} onCancel={onCancelTransfer} />

      {!online && !hosting && (
        // Offline, the notice takes the composer's place — one strip, said once,
        // ending in the one thing that helps: dial them back, or (with no saved
        // address) hand them a way to dial you.
        <div className="composer">
          <div className={cx("composer-notice", redial.failed && "is-failed")} role="status">
            <span className="composer-notice-txt">
              {contact.state === "connecting"
                ? OFFLINE_NOTICE.connecting
                : redial.failed
                  ? <>Couldn't reach {contact.name}{reconnectContact?.address ? ` at ${reconnectContact.address}` : ""}. They need P2PEM open and accepting new peers — or send them your invite link. <span className="composer-notice-why">({redial.failed})</span></>
                  : canRedial
                    ? `${contact.name} is offline. You can write again once you're connected.`
                    : `${contact.name} is offline and there's no saved address to reach them. Send them your invite link so they can connect to you.`}
            </span>
            {contact.state !== "connecting" && (
              <span className="composer-notice-actions">
                {canRedial && (
                  <Button size="sm" icon="refresh" disabled={redial.busy} onClick={doReconnect}>
                    {redial.busy ? "Connecting…" : redial.failed ? "Try again" : "Reconnect"}
                  </Button>
                )}
                {(!canRedial || redial.failed) && (
                  <Button size="sm" variant="ghost" icon="link" onClick={copyInvite}>Copy invite link</Button>
                )}
              </span>
            )}
          </div>
        </div>
      )}

      {online && <div className="composer">
        <div className={cx("composer-box", !online && "is-off")}>
          <button className="composer-clip"
            title={clipTitle} aria-label={clipTitle}
            disabled={!online || sending}
            onClick={() => onSendFile && onSendFile(contact)}><Icon name="paperclip" size={19} /></button>
          <textarea className="composer-input" id="composer-input" rows={1}
            aria-label={online ? `Message ${contact.name}` : "Message box — not connected"}
            placeholder={online ? `Message ${contact.name.split(" ")[0]}` : "Offline — you can write once you're connected"}
            // Typing into a conversation with no session used to look like it
            // worked: the send reported success, the box cleared, and the message
            // simply never existed. Better to refuse the keystroke than to eat a
            // paragraph.
            disabled={!online}
            value={draft} onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); onSend(); } }} />
          <button className={cx("composer-send", online && draft.trim() && "is-ready")}
            onClick={onSend} disabled={!online} title={online ? "Send (Enter)" : "Not connected"}
            aria-label="Send">
            <Icon name="send" size={17} />
          </button>
        </div>
      </div>}

      {riskyOpen && (
        <Modal open onClose={() => setRiskyOpen(null)} width={440}
          title="This file will run as a program" icon="alert">
          <div className="creator-pane">
            <p className="creator-lead">
              <strong>{riskyOpen.filename}</strong> is a{" "}
              <code>.{riskyOpen.kind}</code> file. Opening it doesn't show you
              anything — it hands it to your system to <strong>execute</strong>,
              with your account's access to your files.
            </p>
            <p className="creator-lead">
              It came from <strong>{contact.name}</strong>. Only continue if you
              were expecting a program from them and you trust it. If you're not
              sure, close this and ask them on a call.
            </p>
            <div style={{ display: "flex", gap: 10, justifyContent: "flex-end" }}>
              <Button icon="x" variant="primary" autoFocus onClick={() => setRiskyOpen(null)}>Don't open</Button>
              <Button variant="danger-ghost" icon="alert" onClick={confirmRiskyOpen}>
                Run it anyway
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  );
}
