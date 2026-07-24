// Unified Messages surface — conversation list + chat pane. Clean port of the
// mockup's chat.jsx, driven by live data adapted in lib/bridge.js.
import { useRef, useEffect, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { api, fmtDay } from "../lib/bridge.js";
import { cx, Avatar, IconButton, Button, TrustBadge, TransportBadge } from "./ui.jsx";

const IMAGE_RE = /\.(png|jpe?g|gif|webp|bmp)$/i;
const isImageName = (name) => IMAGE_RE.test(name || "");

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
    // Received files are auto-saved to the configured download directory, so a
    // completed card shows a "saved" state rather than a dead download button.
    const sub = done ? (mine ? "sent" : "saved to Downloads") : m.progress + "%";
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
              title={canOpen ? "Open file" : undefined}>
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
        <span className="msg-text">{m.text}</span>
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

// How many messages render at once. Long histories only mount their tail, so a
// 10k-message thread doesn't re-render thousands of nodes on every poll tick;
// "Show earlier messages" widens the window on demand.
const MSG_WINDOW = 150;

export function ChatPane({ contact, onVerify, onRename, onDelete, onInfo, onSendFile, draft, setDraft, onSend, transfers, onAcceptTransfer, onDeclineTransfer, onCancelTransfer }) {
  const scrollRef = useRef(null);
  const [shown, setShown] = useState(MSG_WINDOW);
  // Inline previews for image files, fetched once per message id (null =
  // asked, not previewable) so the poll-driven re-renders never refetch.
  const [previews, setPreviews] = useState({});
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
      if (!cancelled) setPreviews((p) => ({ ...p, ...updates }));
    })();
    return () => { cancelled = true; };
  }, [contact && contact.id, previewKey, shown]);

  const openFile = (m) => api.openFile(contact.id, m.id, false).catch(() => {});
  const revealFile = (m) => api.openFile(contact.id, m.id, true).catch(() => {});

  if (!contact) {
    return (
      <div className="chat-pane chat-empty">
        <div className="chat-empty-inner">
          <span className="chat-empty-ic"><Icon name="message" size={28} /></span>
          <div className="chat-empty-h">Select a conversation</div>
          <div className="chat-empty-p">Encrypted peer-to-peer messaging. Pick a conversation, or start a new connection.</div>
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

      <div className="composer">
        <button className="composer-clip" title="Send file"
          disabled={contact.state !== "connected"}
          onClick={() => onSendFile && onSendFile(contact)}><Icon name="paperclip" size={19} /></button>
        <textarea className="composer-input" rows={1} placeholder={`Message ${contact.name.split(" ")[0]}…`}
          value={draft} onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); onSend(); } }} />
        <button className={cx("composer-send", draft.trim() && "is-ready")} onClick={onSend} title="Send">
          <Icon name="send" size={18} />
        </button>
      </div>
    </div>
  );
}
