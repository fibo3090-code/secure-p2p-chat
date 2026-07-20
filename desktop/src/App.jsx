import { useCallback, useEffect, useRef, useState } from "react";
import { api, onBridge, summaryToConv, chatToContact } from "./lib/bridge.js";
import { computeUnread } from "./lib/partyUnread.js";
import { Icon } from "./lib/Icon.jsx";
import { cx, Avatar } from "./components/ui.jsx";
import { ConvList, ChatPane } from "./components/Messages.jsx";
import { LockScreen, SetPasswordScreen } from "./components/Onboarding.jsx";
import { Creator } from "./components/Creator.jsx";
import { Verify } from "./components/Verify.jsx";
import { Contacts } from "./components/Contacts.jsx";
import { Settings } from "./components/Settings.jsx";
import { Parties } from "./components/Parties.jsx";
import { Relays } from "./components/Relays.jsx";
import { Toasts } from "./components/Toasts.jsx";
import { RenameDialog, ConfirmDelete, InfoDialog } from "./components/ChatDialogs.jsx";
import { toast } from "./lib/toast.js";
import { THEMES, loadTheme, saveTheme } from "./lib/themes.js";

function RailBtn({ icon, label, active, onClick, badge }) {
  return (
    <button className={cx("rail-btn", active && "is-active")} onClick={onClick} title={label}>
      <Icon name={icon} size={20} />
      <span className="rail-label">{label}</span>
      {badge > 0 && <span className="rail-badge">{badge}</span>}
    </button>
  );
}

function ThemeMenu({ theme, setTheme }) {
  const [open, setOpen] = useState(false);
  const ref = useRef(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);
  return (
    <div className="theme-wrap" ref={ref}>
      <button className="tb-icon" title="Theme" onClick={() => setOpen((o) => !o)}>
        <Icon name="settings" size={16} />
      </button>
      {open && (
        <div className="theme-menu">
          {THEMES.map((t) => (
            <button key={t.id} className={cx("theme-opt", theme === t.id && "is-active")}
              onClick={() => { setTheme(t.id); setOpen(false); }}>
              <span className="theme-dot" style={{ background: t.swatch }} />
              {t.label}
              {theme === t.id && <span className="theme-check"><Icon name="check" size={15} /></span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default function App() {
  const [auth, setAuth] = useState(null);
  const [theme, setThemeState] = useState(loadTheme());
  const [nav, setNav] = useState("chats");
  const [convs, setConvs] = useState([]);
  const [activeId, setActiveId] = useState(null);
  const [active, setActive] = useState(null);
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState("");
  const [creatorOpen, setCreatorOpen] = useState(false);
  const [fpReq, setFpReq] = useState(null);
  const [renameTarget, setRenameTarget] = useState(null);
  const [deleteTarget, setDeleteTarget] = useState(null);
  const [infoTarget, setInfoTarget] = useState(null);
  // Unread bookkeeping: per-conversation message counts already seen (null until
  // seeded from the first list load), plus a ref mirror of the active id so the
  // event-driven refresh never reads a stale closure.
  const seenRef = useRef(null);
  const activeIdRef = useRef(null);
  const [transfers, setTransfers] = useState([]);
  const [partyUnread, setPartyUnread] = useState(0);

  const setTheme = useCallback((id) => { setThemeState(id); saveTheme(id); }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    document.documentElement.setAttribute("data-density", "regular");
  }, [theme]);

  const refreshAuth = useCallback(async () => {
    try { setAuth(await api.authStatus()); } catch { /* bridge not ready */ }
  }, []);
  useEffect(() => { refreshAuth(); }, [refreshAuth]);

  const ready = auth?.state === "ready";

  const refresh = useCallback(async () => {
    if (!ready) return;
    try {
      const list = await api.listConversations();
      // Unread bookkeeping (mirrors the TUI): seed the per-conversation "seen"
      // counts on first load so old history never shows as unread, keep the open
      // conversation always read, and badge anything beyond the seen count.
      if (seenRef.current === null) {
        seenRef.current = Object.fromEntries(list.map((c) => [c.id, c.messages ?? 0]));
      }
      const seen = seenRef.current;
      const cur = activeIdRef.current;
      if (cur) {
        const act = list.find((c) => c.id === cur);
        if (act) seen[cur] = act.messages ?? 0;
      }
      setConvs(list.map((c) => ({ ...c, unread: Math.max(0, (c.messages ?? 0) - (seen[c.id] ?? 0)) })));
      // Live file-transfer progress, shown in the chat pane.
      api.listTransfers().then(setTransfers).catch(() => {});
      // Surface any pending TOFU prompt even if its one-shot event was missed.
      api.pendingFingerprint().then((p) => { if (p) setFpReq((cur) => cur || p); }).catch(() => {});
      setActiveId((cur) => {
        if (cur) {
          api.getConversation(cur).then((chat) => {
            setActive(chatToContact(chat, list.find((c) => c.id === cur)?.connected));
          }).catch(() => {});
        }
        return cur;
      });
    } catch { /* ignore */ }
  }, [ready]);

  useEffect(() => {
    if (!ready) return;
    refresh();
    const u1 = onBridge("state-updated", () => refresh());
    const u2 = onBridge("fingerprint-request", (e) => setFpReq(e.payload));
    const u3 = onBridge("toast", (e) => toast(e.payload.message, e.payload.level));
    // Communities unread total for the rail badge — computed here (not in the
    // Parties pane) so messages arriving while another tab is open still badge.
    const u4 = onBridge("party-updated", async () => {
      try { setPartyUnread(computeUnread(await api.partyList()).total); } catch { /* ignore */ }
    });
    return () => { u1.then((f) => f()); u2.then((f) => f()); u3.then((f) => f()); u4.then((f) => f()); };
  }, [ready, refresh]);

  if (!auth) return null;

  if (auth.state === "unlock") {
    return <LockScreen onUnlock={async (pw) => {
      try { await api.unlock(pw); await refreshAuth(); return ""; } catch (e) { return String(e); }
    }} />;
  }
  if (auth.state === "set_password") {
    return <SetPasswordScreen fingerprint={auth.fingerprint} onSet={async (pw) => {
      try { await api.setPassword(pw); await refreshAuth(); return ""; } catch (e) { return String(e); }
    }} />;
  }

  const contacts = convs.map(summaryToConv);
  const connCount = convs.filter((c) => c.connected).length;

  async function openConv(id) {
    setActiveId(id);
    activeIdRef.current = id;
    // Opening a conversation reads it: sync the seen count and clear its badge
    // immediately (the next poll would do it anyway, this avoids a flicker).
    if (seenRef.current) {
      const c = convs.find((x) => x.id === id);
      if (c) seenRef.current[id] = c.messages ?? 0;
    }
    setConvs((cs) => cs.map((c) => (c.id === id ? { ...c, unread: 0 } : c)));
    setNav("chats");
    // Drafts are per-conversation: clear on switch so unsent text can't follow
    // the user into another thread and be sent to the wrong recipient.
    setDraft("");
    try {
      const chat = await api.getConversation(id);
      setActive(chatToContact(chat, convs.find((c) => c.id === id)?.connected));
    } catch { /* ignore */ }
  }

  async function send() {
    const text = draft.trim();
    if (!text || !activeId) return;
    setDraft("");
    // Restore the draft if the send fails, so a transient error doesn't lose
    // what the user typed — they can just press Enter again.
    try { await api.sendMessage(activeId, text); }
    catch (e) { setDraft(text); toast(String(e), "error"); return; }
    refresh();
  }

  async function sendFile(c) {
    try { await api.sendFile(c.id); refresh(); }
    catch (e) { toast(String(e), "error"); }
  }

  async function doRename(id, title) {
    setRenameTarget(null);
    try { await api.renameChat(id, title); toast("Conversation renamed", "success"); refresh(); }
    catch (e) { toast(String(e), "error"); }
  }

  async function doDelete(id) {
    setDeleteTarget(null);
    try {
      await api.deleteChat(id);
      if (activeId === id) { setActiveId(null); activeIdRef.current = null; setActive(null); }
      toast("Conversation deleted", "success");
      refresh();
    } catch (e) { toast(String(e), "error"); }
  }

  return (
    <div className="app-root">
      <div className="hex-bg" />
      <header className="titlebar">
        <div className="tb-left">
          <span className="tb-brand"><Icon name="shieldCheck" size={16} /> P2PEM</span>
          <span className={cx("tb-status", connCount && "is-on")}>
            <span className="tb-status-dot" />
            {connCount ? `${connCount} peer${connCount > 1 ? "s" : ""} connected` : "No active peers"}
          </span>
        </div>
        <div className="tb-right">
          <button className="tb-btn" onClick={() => setCreatorOpen(true)}>
            <Icon name="plus" size={15} /> New connection
          </button>
          <ThemeMenu theme={theme} setTheme={setTheme} />
        </div>
      </header>

      <div className="app-body">
        <nav className="rail">
          <div className="rail-nav">
            <RailBtn icon="message" label="Chats" active={nav === "chats"} onClick={() => setNav("chats")}
              badge={convs.reduce((n, c) => n + (c.unread || 0), 0)} />
            <RailBtn icon="users" label="Communities" active={nav === "parties"} onClick={() => setNav("parties")}
              badge={partyUnread} />
            <RailBtn icon="server" label="Relays" active={nav === "relays"} onClick={() => setNav("relays")} />
            <RailBtn icon="user" label="Contacts" active={nav === "contacts"} onClick={() => setNav("contacts")} />
            <RailBtn icon="settings" label="Settings" active={nav === "settings"} onClick={() => setNav("settings")} />
          </div>
          <div className="rail-foot">
            <button className="rail-id" title={auth.name}><Avatar name={auth.name} size={34} /></button>
          </div>
        </nav>

        {nav === "chats" && (
          <>
            <aside className="col-list">
              <ConvList contacts={contacts} activeId={activeId} onSelect={openConv}
                onAdd={() => setCreatorOpen(true)} query={query} setQuery={setQuery} />
            </aside>
            <main className="col-main">
              <ChatPane contact={active} draft={draft} setDraft={setDraft} onSend={send}
                onSendFile={sendFile}
                transfers={transfers.filter((t) =>
                  // done/cancelled rows can linger in the snapshot until the next
                  // transfer; the completed file already shows as a message, so
                  // only surface in-flight work and failures (with their reason).
                  t.chat_id === activeId && t.status !== "done" && t.status !== "cancelled")}
                onAcceptTransfer={async (t) => {
                  try { await api.acceptTransfer(t.id); refresh(); } catch (e) { toast(String(e), "error"); }
                }}
                onDeclineTransfer={async (t) => {
                  try { await api.declineTransfer(t.id); refresh(); } catch (e) { toast(String(e), "error"); }
                }}
                onVerify={async (c) => {
                  // Open the accept/reject dialog only when a real TOFU request
                  // is pending for this chat; an established chat has nothing to
                  // confirm, so show the read-only fingerprint instead.
                  try {
                    const p = await api.pendingFingerprint();
                    if (p && p.chat_id === c.id) { setFpReq(p); return; }
                  } catch { /* ignore */ }
                  setInfoTarget(c);
                }}
                onRename={(c) => setRenameTarget(c)}
                onDelete={(c) => setDeleteTarget(c)}
                onInfo={(c) => setInfoTarget(c)} />
            </main>
          </>
        )}
        {nav === "contacts" && (
          <main className="col-full"><Contacts onConnected={() => { setNav("chats"); refresh(); }} /></main>
        )}
        {nav === "settings" && (
          <main className="col-full"><Settings identity={auth} theme={theme} setTheme={setTheme} /></main>
        )}
        {nav === "parties" && <main className="col-full"><Parties /></main>}
        {nav === "relays" && <main className="col-full"><Relays onConnected={() => { setNav("chats"); refresh(); }} /></main>}
      </div>

      <Creator open={creatorOpen} onClose={() => setCreatorOpen(false)} />
      <Verify req={fpReq} onClose={() => setFpReq(null)} />
      <RenameDialog target={renameTarget} onClose={() => setRenameTarget(null)} onSubmit={doRename} />
      <ConfirmDelete target={deleteTarget} onClose={() => setDeleteTarget(null)} onConfirm={doDelete} />
      <InfoDialog target={infoTarget} onClose={() => setInfoTarget(null)} />
      <Toasts />
    </div>
  );
}
