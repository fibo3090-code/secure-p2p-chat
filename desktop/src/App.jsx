import { useCallback, useEffect, useRef, useState } from "react";
import { api, onBridge, summaryToConv, chatToContact } from "./lib/bridge.js";
import { computeUnread } from "./lib/partyUnread.js";
import { Icon } from "./lib/Icon.jsx";
import { cx, Avatar } from "./components/ui.jsx";
import { ConvList, ChatPane } from "./components/Messages.jsx";
import { BootScreen, LockScreen, SetPasswordScreen, BackupPrompt } from "./components/Onboarding.jsx";
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

// Backoff for the startup `auth_status` handshake: quick first retries so a
// slow bridge is invisible, then back off so a hard failure doesn't spin.
const BOOT_RETRY_MS = [400, 800, 1600, 3200, 6400, 10000];

export default function App() {
  const [auth, setAuth] = useState(null);
  // Boot state. `auth === null` used to render nothing at all, so a single
  // failed or slow `auth_status` left a permanently blank window with no
  // message, no spinner and no recovery. Now the failure is visible and retried.
  const [bootError, setBootError] = useState("");
  const [bootAttempt, setBootAttempt] = useState(0);
  // Shown once, right after a new identity is created.
  const [backupPrompt, setBackupPrompt] = useState(false);
  const [theme, setThemeState] = useState(loadTheme());
  const [nav, setNav] = useState("chats");
  const [convs, setConvs] = useState([]);
  const [activeId, setActiveId] = useState(null);
  const [active, setActive] = useState(null);
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState("");
  const [creatorOpen, setCreatorOpen] = useState(false);
  // Which tab the connection dialog opens on, so the get-started suggestions
  // land the user on the path they picked instead of a generic dialog.
  const [creatorMode, setCreatorMode] = useState("connect");
  const [fpReq, setFpReq] = useState(null);
  const [renameTarget, setRenameTarget] = useState(null);
  const [deleteTarget, setDeleteTarget] = useState(null);
  const [infoTarget, setInfoTarget] = useState(null);
  // Unread is computed by the bridge from a read mark persisted inside the
  // encrypted history, so a message that arrived while the app was closed is
  // still unread on the next launch. The refs mirror the open conversation and
  // visibility so the event-driven refresh never reads a stale closure.
  const activeIdRef = useRef(null);
  const visibleRef = useRef(true);
  const [transfers, setTransfers] = useState([]);
  const [partyUnread, setPartyUnread] = useState(0);
  // Conversation lock: when on, no new peer can connect (listener stopped,
  // auto-rehost paused). Existing sessions keep running.
  const [locked, setLockedState] = useState(false);
  // Does the user actually have the window in front of them? Pushed down to the
  // bridge so "notify when a message arrives in the background" means it.
  // `visible` is tracked separately because it is the far more reliable of the
  // two signals — see the effect below.
  const [focused, setFocused] = useState(true);
  const [visible, setVisible] = useState(true);

  const setTheme = useCallback((id) => { setThemeState(id); saveTheme(id); }, []);
  const openCreator = useCallback((mode = "connect") => {
    setCreatorMode(mode);
    setCreatorOpen(true);
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    document.documentElement.setAttribute("data-density", "regular");
  }, [theme]);

  const refreshAuth = useCallback(async () => {
    const status = await api.authStatus();
    setAuth(status);
    setBootError("");
    return status;
  }, []);

  // Keep trying until the bridge answers. Every failure is shown, so the user
  // knows the app is working on it rather than staring at an empty window.
  useEffect(() => {
    if (auth) return undefined;
    let cancelled = false;
    let timer;
    (async () => {
      try {
        await refreshAuth();
      } catch (e) {
        if (cancelled) return;
        setBootError(String(e?.message || e) || "The backend did not respond.");
        const delay = BOOT_RETRY_MS[Math.min(bootAttempt, BOOT_RETRY_MS.length - 1)];
        timer = setTimeout(() => !cancelled && setBootAttempt((n) => n + 1), delay);
      }
    })();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [auth, bootAttempt, refreshAuth]);

  // Window focus + visibility, tracked separately because they are trusted
  // differently:
  //
  // - `visible` (visibilityState) is reliable across webviews and is what gates
  //   clearing unread. If it were gated on focus instead, a webview that
  //   under-reports `hasFocus()` would leave conversations badged forever —
  //   a worse failure than an occasional extra notification.
  // - `focused` (visible AND hasFocus) is what the notification gate uses:
  //   suppressing a popup needs *more* confidence that the user is looking.
  useEffect(() => {
    const update = () => {
      const vis = document.visibilityState === "visible";
      const foc = vis && document.hasFocus();
      visibleRef.current = vis;
      setVisible(vis);
      setFocused(foc);
    };
    update();
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    return () => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
    };
  }, []);

  const ready = auth?.state === "ready";

  // Tell the bridge what is on screen. ChatManager owns no window handle, so
  // without this it cannot tell a background message from one the user is
  // reading right now.
  useEffect(() => {
    if (!ready) return;
    api.setPresence(focused, activeId).catch(() => {});
  }, [ready, focused, activeId]);

  const refresh = useCallback(async () => {
    if (!ready) return;
    try {
      const list = await api.listConversations();
      // `unread` comes from the bridge (a persisted read mark), so it is already
      // correct across restarts. The only local adjustment is optimistic: the
      // conversation the user is looking at right now reads as zero immediately,
      // instead of flashing a badge until `mark_read` round-trips.
      const cur = activeIdRef.current;
      setConvs(list.map((c) => (
        c.id === cur && visibleRef.current ? { ...c, unread: 0 } : c
      )));
      // Live file-transfer progress, shown in the chat pane.
      api.listTransfers().then(setTransfers).catch(() => {});
      // The pending TOFU prompt is authoritative state on the bridge, which
      // holds a queue: adopt whichever is at the head (so a second peer's
      // prompt is surfaced once the first is answered, instead of being lost),
      // and drop ours when there is none left — a prompt whose session already
      // died can never be answered, and an undismissable dialog for it would
      // wedge the app.
      api.pendingFingerprint()
        .then((p) => setFpReq((cur) => (p && cur && cur.chat_id === p.chat_id ? cur : p)))
        .catch(() => {});
      api.lockState().then(setLockedState).catch(() => {});
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

  // Clear the read mark for the conversation the user is actually looking at.
  // Keyed on that conversation's message count so an arrival in the open chat
  // is marked read too, without firing on every idle poll.
  const activeMessages = convs.find((c) => c.id === activeId)?.messages ?? 0;
  useEffect(() => {
    if (!ready || !activeId || !visible) return;
    api.markRead(activeId).catch(() => {});
  }, [ready, activeId, visible, activeMessages]);

  if (!auth) {
    return (
      <BootScreen error={bootError} retrying={!bootError}
        onRetry={() => { setBootError(""); setBootAttempt((n) => n + 1); }} />
    );
  }

  // Startup found an identity file it could not read. The backend refuses to
  // replace it (that would abandon the user's history and their contacts'
  // trust), so there is nothing to retry — explain and stop.
  if (auth.state === "error") {
    return <BootScreen fatal error={auth.error || "The identity could not be loaded."} />;
  }

  if (auth.state === "unlock") {
    return <LockScreen onUnlock={async (pw) => {
      try { await api.unlock(pw); await refreshAuth(); return ""; } catch (e) { return String(e); }
    }} />;
  }
  if (auth.state === "set_password") {
    return <SetPasswordScreen fingerprint={auth.fingerprint} minLength={auth.min_password_len}
      onSet={async (pw) => {
        try {
          await api.setPassword(pw);
          await refreshAuth();
          // A brand-new identity is exactly when a backup matters and exactly
          // when nobody thinks to make one — so ask here, once, with the action
          // attached rather than only a warning that it can't be reset.
          setBackupPrompt(true);
          return "";
        } catch (e) { return String(e); }
      }} />;
  }
  if (backupPrompt) {
    return <BackupPrompt onSkip={() => setBackupPrompt(false)}
      onExport={() => api.exportIdentity()} />;
  }

  const contacts = convs.map(summaryToConv);
  const connCount = convs.filter((c) => c.connected).length;

  async function openConv(id) {
    setActiveId(id);
    activeIdRef.current = id;
    // Opening a conversation reads it. Clear the badge locally for an instant
    // response; `mark_read` persists it so the badge stays cleared next launch.
    setConvs((cs) => cs.map((c) => (c.id === id ? { ...c, unread: 0 } : c)));
    api.markRead(id).catch(() => {});
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
          <button className={cx("tb-icon", locked && "is-locked")}
            title={locked
              ? "Conversation locked: no new peer can connect. Click to unlock."
              : "Lock the conversation: stop listening and refuse new peers."}
            onClick={async () => {
              try { await api.setLocked(!locked); setLockedState(!locked); toast(!locked ? "Locked — no new peers can connect" : "Unlocked", "info"); }
              catch (e) { toast(String(e), "error"); }
            }}>
            <Icon name={locked ? "lock" : "unlock"} size={16} />
          </button>
          <button className="tb-btn" onClick={() => openCreator()}>
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
            <button className={cx("rail-id", nav === "settings" && "is-active")}
              title={`${auth.name} — open your identity settings`}
              aria-label={`${auth.name} — open your identity settings`}
              onClick={() => setNav("settings")}>
              <Avatar name={auth.name} size={34} />
            </button>
          </div>
        </nav>

        {nav === "chats" && (
          <>
            <aside className="col-list">
              <ConvList contacts={contacts} activeId={activeId} onSelect={openConv}
                onAdd={() => openCreator()} query={query} setQuery={setQuery} />
            </aside>
            <main className="col-main">
              <ChatPane contact={active} draft={draft} setDraft={setDraft} onSend={send}
                onSendFile={sendFile}
                // A user with no conversations at all gets the get-started
                // panel rather than "select a conversation" with nothing to select.
                isFirstRun={convs.length === 0}
                onStart={openCreator}
                // A swallowed failure here is a cancel button that does
                // nothing, on a transfer the user is actively trying to stop.
                onCancelTransfer={(id) =>
                  api.cancelTransfer(id).catch((e) => toast(`Could not cancel: ${e}`, "error"))}
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
          <main className="col-full"><Settings identity={auth} theme={theme} setTheme={setTheme} onIdentityChanged={refreshAuth} /></main>
        )}
        {nav === "parties" && <main className="col-full"><Parties /></main>}
        {nav === "relays" && <main className="col-full"><Relays onConnected={() => { setNav("chats"); refresh(); }} /></main>}
      </div>

      <Creator open={creatorOpen} initialMode={creatorMode} onClose={() => setCreatorOpen(false)} />
      <Verify req={fpReq} onClose={() => setFpReq(null)} />
      <RenameDialog target={renameTarget} onClose={() => setRenameTarget(null)} onSubmit={doRename} />
      <ConfirmDelete target={deleteTarget} onClose={() => setDeleteTarget(null)} onConfirm={doDelete} />
      <InfoDialog target={infoTarget} onClose={() => setInfoTarget(null)} />
      <Toasts />
    </div>
  );
}
