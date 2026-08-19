// Unified Messages surface — conversation list + chat pane. Clean port of the
// mockup's chat.jsx, driven by live data adapted in lib/bridge.js.
import { useRef, useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { api, fmtDay } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";
import { cx, Avatar, IconButton, Button, Modal, TrustBadge, TransportBadge } from "./ui.jsx";

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
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);
  const item = (fn) => () => { setOpen(false); fn && fn(contact); };
  return (
    <div className="chat-head-actions">
      <IconButton name="fingerprint" label="Verify fingerprint" onClick={() => onVerify && onVerify(contact)} />
      <div className="pop-wrap" ref={ref}>
        <IconButton name="more" label="More" active={open} onClick={() => setOpen((o) => !o)} />
        {open && (
          <div className="pop-menu">
            <button onClick={item(onInfo)}><Icon name="info" size={15} /> Conversation info</button>
            <button onClick={item(onRename)}><Icon name="edit" size={15} /> Rename</button>
            <button className="is-danger" onClick={item(onDelete)}><Icon name="trash" size={15} /> Delete</button>
          </div>
        )}
      </div>
    </div>
  );
}

const STATE_LABEL = { connected: "Connected", hosting: "Hosting", offline: "Offline", connecting: "Connecting" };

// Shown above a disabled composer. Messages only exist once a session carries
// them, so say which situation this is and what would fix it — the alternative
// (accept the text and drop it) is how a typed paragraph disappears.
const OFFLINE_NOTICE = {
  offline: "Not connected — messages can't be sent yet. Reconnect to this peer, or ask them to connect to you.",
  hosting: "Waiting for a peer to connect. Share your address, then you can start the conversation.",
  connecting: "Connecting… you'll be able to send as soon as the peer answers.",
};

export function ConvList({ contacts, activeId, onSelect, onAdd, query, setQuery }) {
  const filtered = contacts.filter((c) => c.name.toLowerCase().includes(query.toLowerCase()));
  return (
    <div className="conv-list">
      <div className="conv-search">
        <Icon name="search" size={15} />
        <input placeholder="Search conversations" value={query} onChange={(e) => setQuery(e.target.value)} />
        <button className="conv-add" onClick={onAdd} title="New connection"><Icon name="plus" size={17} /></button>
      </div>
      <div className="conv-scroll">
        {filtered.map((c) => (
          <button key={c.id} className={cx("conv-row", activeId === c.id && "is-active")} onClick={() => onSelect(c.id)}>
            <Avatar name={c.name} size={42} state={c.state} party={c.trust === "party"} />
            <div className="conv-main">
              <div className="conv-line1">
                <span className="conv-name">{c.name}</span>
                <span className="conv-time">{c.lastT}</span>
              </div>
              <div className="conv-line2">
                <span className="conv-preview">
                  {c.typing ? <em className="conv-typing">typing…</em> : c.last}
                </span>
                <span className="conv-markers">
                  {c.kind === "group" && <Icon name="users" size={12} />}
                  {c.kind === "channel" && <Icon name="hash" size={12} />}
                  {c.transport === "relay" && <Icon name="satellite" size={12} />}
                  {c.transport === "server" && <Icon name="server" size={12} />}
                  {c.state === "hosting" && <span className="marker-h">H</span>}
                  {c.trust === "unverified" && <span className="marker-warn"><Icon name="alert" size={12} /></span>}
                  {c.unread > 0 && <span className="conv-unread">{c.unread}</span>}
                </span>
              </div>
            </div>
          </button>
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

function MessageItem({ m, preview, onOpen, onReveal }) {
  if (m.kind === "system") {
    return (
      <div className={cx("sys-msg", m.warn && "is-warn", m.ok && "is-ok")}>
        <Icon name={m.warn ? "alert" : m.ok ? "shieldCheck" : "lock"} size={13} />
        <span>{m.text}</span>
        <span className="sys-t">{m.t}</span>
      </div>
    );
  }
  if (m.kind === "file") {
    const mine = m.from === "me";
    const done = m.progress >= 100;
    // Name the folder the file actually went to. Hardcoding "Downloads" was
    // wrong for anyone who picked a different download folder — and this card
    // is the only place the app ever says where a received file landed.
    const sub = done
      ? (mine ? "sent" : m.dir ? `saved to ${m.dir}` : "saved")
      : m.progress + "%";
    const canOpen = done && m.hasPath;
    const open = canOpen && onOpen ? () => onOpen(m) : undefined;
    return (
      <div className={cx("msg-row", mine ? "is-mine" : "is-them")}>
        <div className={cx("file-card", mine && "is-mine", canOpen && "is-openable")}>
          {preview && (
            <button className="file-thumb" onClick={open} title={"Open " + m.name}>
              <img src={preview} alt={m.name} loading="lazy" />
            </button>
          )}
          <div className="file-card-row">
            <button className="file-ic" onClick={open} disabled={!canOpen}
              title={canOpen ? "Open file" : undefined}><Icon name="file" size={20} /></button>
            <button className="file-meta" onClick={open} disabled={!canOpen}
              title={m.path || (canOpen ? "Open file" : undefined)}>
              <div className="file-name">{m.name}</div>
              <div className="file-sub">{m.size} · {sub} · {m.t}</div>
            </button>
            {canOpen && (
              <button className="file-reveal" title="Show in folder"
                onClick={() => onReveal && onReveal(m)}><Icon name="folder" size={15} /></button>
            )}
            <span className={cx("file-status", done && "is-done")} title={sub}>
              <Icon name={done ? "check" : "clock"} size={16} />
            </span>
          </div>
        </div>
      </div>
    );
  }
  const mine = m.from === "me";
  return (
    <div className={cx("msg-row", mine ? "is-mine" : "is-them")}>
      <div className="msg-bubble">
        {m.author && !mine && <span className="msg-author">{m.author}</span>}
        <span className="msg-text"><LinkifiedText text={m.text} /></span>
        <span className="msg-time">{m.t}{mine && m.delivered && <Icon name="check" size={12} />}</span>
      </div>
    </div>
  );
}

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

export function ChatPane({ contact, onVerify, onRename, onDelete, onInfo, onSendFile, draft, setDraft, onSend, transfers, onAcceptTransfer, onDeclineTransfer, onCancelTransfer, isFirstRun, onStart }) {
  const scrollRef = useRef(null);
  const [shown, setShown] = useState(MSG_WINDOW);
  // Inline previews for image files, fetched once per message id (null =
  // asked, not previewable) so the poll-driven re-renders never refetch.
  const [previews, setPreviews] = useState({});
  // A peer-sent file the OS would execute, held for the user's confirmation.
  const [riskyOpen, setRiskyOpen] = useState(null);
  const requested = useRef(new Set());
  useEffect(() => {
    setShown(MSG_WINDOW);
    setPreviews({});
    requested.current = new Set();
  }, [contact && contact.id]);
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [contact && contact.id, contact && contact.messages.length]);
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
    let cancelled = false;
    (async () => {
      const updates = {};
      for (const m of want) {
        try { updates[m.id] = await api.filePreview(contact.id, m.id); }
        catch { updates[m.id] = null; }
      }
      // Bounded: `requested` still remembers what was asked for, so an evicted
      // preview is not re-fetched in a loop — it simply renders as a plain card
      // until the conversation is reopened.
      if (!cancelled) setPreviews((p) => trimPreviews({ ...p, ...updates }));
    })();
    return () => { cancelled = true; };
  }, [contact && contact.id, previewKey, shown]);

  // Opening a peer's file can mean running their code. The bridge refuses
  // those unless confirmed and tells us what it is, so the click turns into a
  // question instead of an execution.
  const openFile = async (m) => {
    try {
      const r = await api.openFile(contact.id, m.id, false);
      if (r && r.blocked) setRiskyOpen({ msg: m, kind: r.blocked, filename: r.filename || m.name });
    } catch (e) { toast(String(e), "error"); }
  };
  const confirmRiskyOpen = async () => {
    const target = riskyOpen;
    setRiskyOpen(null);
    if (!target) return;
    try { await api.openFile(contact.id, target.msg.id, false, true); }
    catch (e) { toast(String(e), "error"); }
  };
  // Revealing never launches anything, so it is never gated.
  const revealFile = (m) =>
    api.openFile(contact.id, m.id, true)
      .catch((e) => toast(`Could not show the file: ${e}`, "error"));

  // Sending needs a live session. Without this the composer accepted a whole
  // paragraph, the bridge reported success, and the text vanished.
  const online = contact ? contact.state === "connected" : false;
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

  if (!contact) {
    // With nothing to select, "Select a conversation" is a dead end — and this
    // is exactly the moment a new user decides whether the app works. There are
    // no accounts and no directory here, so the first connection genuinely needs
    // explaining; show the three real paths instead of a shrug.
    if (isFirstRun) return <GetStarted onStart={onStart} />;
    return (
      <div className="chat-pane chat-empty">
        <div className="chat-empty-inner">
          <span className="chat-empty-ic"><Icon name="message" size={28} /></span>
          <div className="chat-empty-h">Select a conversation</div>
          <div className="chat-empty-p">Pick a conversation on the left, or start a new connection.</div>
        </div>
      </div>
    );
  }

  return (
    <div className="chat-pane">
      <header className="chat-head">
        <div className="chat-head-l">
          <Avatar name={contact.name} size={40} state={contact.state} party={contact.trust === "party"} />
          <div className="chat-head-info">
            <div className="chat-head-name">
              {contact.name}
              <TrustBadge trust={contact.trust} mini />
              <TransportBadge transport={contact.transport} kind={contact.kind} mini />
            </div>
            <div className="chat-head-sub mono">
              <span className={"chat-state state-txt-" + contact.state}>{STATE_LABEL[contact.state]}</span>
              {contact.address && <><span className="chat-dot">·</span>{contact.address}</>}
            </div>
          </div>
        </div>
        <ChatMenu contact={contact} onVerify={onVerify} onRename={onRename} onDelete={onDelete} onInfo={onInfo} />
      </header>

      <div className="chat-scroll" ref={scrollRef}>
        <div className="chat-thread">
          {contact.messages.length > shown && (
            <button className="thread-more" onClick={() => setShown((s) => s + MSG_WINDOW)}>
              Show earlier messages ({contact.messages.length - shown} more)
            </button>
          )}
          {(() => {
            const visible = contact.messages.slice(-shown);
            let lastDay = null;
            return visible.map((m, i) => {
              const day = m.ts ? new Date(m.ts).toDateString() : null;
              const sep = day && day !== lastDay ? fmtDay(m.ts) : null;
              if (day) lastDay = day;
              return (
                <div key={m.id || i}>
                  {sep && <div className="day-sep"><span>{sep}</span></div>}
                  <MessageItem m={m} preview={m.id ? previews[m.id] : null}
                    onOpen={openFile} onReveal={revealFile} />
                </div>
              );
            });
          })()}
        </div>
      </div>

      <TransferBar transfers={transfers} onAccept={onAcceptTransfer} onDecline={onDeclineTransfer} onCancel={onCancelTransfer} />

      {!online && (
        <div className="composer-notice" role="status">
          <Icon name="alert" size={14} />
          <span>{OFFLINE_NOTICE[contact.state] || OFFLINE_NOTICE.offline}</span>
        </div>
      )}

      <div className="composer">
        <button className="composer-clip"
          title={clipTitle}
          disabled={!online || sending}
          onClick={() => onSendFile && onSendFile(contact)}><Icon name="paperclip" size={19} /></button>
        <textarea className="composer-input" rows={1}
          placeholder={online ? `Message ${contact.name.split(" ")[0]}…` : "Not connected — reconnect to send"}
          // Typing into a conversation with no session used to look like it
          // worked: the send reported success, the box cleared, and the message
          // simply never existed. Better to refuse the keystroke than to eat a
          // paragraph.
          disabled={!online}
          value={draft} onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); onSend(); } }} />
        <button className={cx("composer-send", online && draft.trim() && "is-ready")}
          onClick={onSend} disabled={!online} title={online ? "Send" : "Not connected"}>
          <Icon name="send" size={18} />
        </button>
      </div>

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
              <Button icon="x" onClick={() => setRiskyOpen(null)}>Don't open</Button>
              <Button variant="danger" icon="alert" onClick={confirmRiskyOpen}>
                Run it anyway
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  );
}
