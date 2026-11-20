import { useCallback, useEffect, useRef, useState } from "react";
import { api, onBridge, summaryToConv, chatToContact, friendlyError } from "./lib/bridge.js";
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
import { HistoryBanner } from "./components/HistoryBanner.jsx";
import { RenameDialog, ConfirmDelete, InfoDialog } from "./components/ChatDialogs.jsx";
import { toast } from "./lib/toast.js";
import { THEMES, loadTheme, saveTheme } from "./lib/themes.js";

function RailBtn({ icon, label, active, onClick, badge }) {
  // Fold the badge count into the accessible name; hide the visual duplicate.
  const unreadLabel = badge > 0 ? `, ${badge} unread` : "";
  return (
    <button className={cx("rail-btn", active && "is-active")} onClick={onClick} title={label}
      aria-label={`${label}${unreadLabel}`} aria-current={active ? "page" : undefined}>
      <Icon name={icon} size={20} />
      <span className="rail-label">{label}</span>
      {badge > 0 && <span className="rail-badge" aria-hidden="true">{badge}</span>}
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

// Backoff for the startup `auth_status` handshake: quick first, then slower.
const BOOT_RETRY_MS = [400, 800, 1600, 3200, 6400, 10000];

export default function App() {
  const [auth, setAuth] = useState(null);
  // Boot state: a failed or slow `auth_status` is shown and retried, never a
  // blank window.
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
  // Which tab the connection dialog opens on (set by the get-started paths).
  const [creatorMode, setCreatorMode] = useState("connect");
  const [fpReq, setFpReq] = useState(null);
  const [renameTarget, setRenameTarget] = useState(null);
  const [deleteTarget, setDeleteTarget] = useState(null);
  const [infoTarget, setInfoTarget] = useState(null);
  // Unread comes from the bridge's persisted read mark. The refs mirror the
  // open conversation and visibility for the event-driven refresh.
  const activeIdRef = useRef(null);
  const visibleRef = useRef(true);
  const [transfers, setTransfers] = useState([]);
  const [partyUnread, setPartyUnread] = useState(0);
  // Conversation lock: no new peer can connect; existing sessions keep running.
  const [locked, setLockedState] = useState(false);
  // Whether the user has the window in front of them, pushed to the bridge for
  // notifications (`visible` is tracked separately; see below).
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

  // Keep trying until the bridge answers, showing every failure.
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

  // `visible` (visibilityState, reliable) gates clearing unread, so a webview
  // under-reporting `hasFocus()` cannot leave conversations badged forever.
  // `focused` (visible AND hasFocus) gates notifications, which need more
  // confidence that the user is looking.
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

  // An unreadable history file blocks saving: ask once per unlock, keep the
  // banner up until resolved.
  const [historyIssue, setHistoryIssue] = useState(null);
  useEffect(() => {
    if (!ready) return;
    api.historyStatus()
      .then((s) => setHistoryIssue(s && s.error ? s : null))
      .catch(() => {});
  }, [ready]);

  // Tell the bridge what is on screen (ChatManager owns no window handle).
  useEffect(() => {
    if (!ready) return;
    api.setPresence(focused, activeId).catch(() => {});
  }, [ready, focused, activeId]);

  const refresh = useCallback(async () => {
    if (!ready) return;
    try {
      const list = await api.listConversations();
      // Optimistic: the open conversation reads as zero before `mark_read`
      // round-trips.
      const cur = activeIdRef.current;
      setConvs(list.map((c) => (
        c.id === cur && visibleRef.current ? { ...c, unread: 0 } : c
      )));
      // Live file-transfer progress, shown in the chat pane.
      api.listTransfers().then(setTransfers).catch(() => {});
      // The bridge queues TOFU prompts: adopt the head, and drop ours when the
      // queue is empty (a dead session's dialog could never be answered).
      api.pendingFingerprint()
        .then((p) => setFpReq((cur) => (p && cur && cur.chat_id === p.chat_id ? cur : p)))
        .catch(() => {});
      api.lockState().then(setLockedState).catch(() => {});
      setActiveId((cur) => {
        if (cur) {
          api.getConversation(cur).then((chat) => {
            // The user may have switched meanwhile.
            if (activeIdRef.current !== cur) return;
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
    // Only transfer progress moved: refetch transfers only (PERF-01).
    const u5 = onBridge("transfers-updated", () => {
      api.listTransfers().then(setTransfers).catch(() => {});
    });
    const u2 = onBridge("fingerprint-request", (e) => setFpReq(e.payload));
    const u3 = onBridge("toast", (e) => toast(e.payload.message, e.payload.level));
    // Communities unread for the rail badge, computed here so it badges on
    // any tab.
    const u4 = onBridge("party-updated", async () => {
      try { setPartyUnread(computeUnread(await api.partyList()).total); } catch { /* ignore */ }
    });
    return () => { [u1, u2, u3, u4, u5].forEach((u) => u.then((f) => f())); };
  }, [ready, refresh]);

  // Mark the visible conversation read, keyed on its message count so an
  // arrival there is marked too without firing every idle poll.
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

  // The identity file could not be read and the backend refuses to replace
  // it: nothing to retry, so explain and stop.
  if (auth.state === "error") {
    return <BootScreen fatal error={auth.error || "The identity could not be loaded."} />;
  }

  if (auth.state === "unlock") {
    return <LockScreen onUnlock={async (pw) => {
      try { await api.unlock(pw); await refreshAuth(); return ""; } catch (e) { return friendlyError(e); }
    }} />;
  }
  if (auth.state === "set_password") {
    return <SetPasswordScreen fingerprint={auth.fingerprint} minLength={auth.min_password_len}
      onSet={async (pw) => {
        try {
          await api.setPassword(pw);
          await refreshAuth();
          // A new identity is when a backup matters most: ask once, with the action.
          setBackupPrompt(true);
          return "";
        } catch (e) { return friendlyError(e); }
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
    // Clear the badge locally now; `mark_read` persists it.
    setConvs((cs) => cs.map((c) => (c.id === id ? { ...c, unread: 0 } : c)));
    api.markRead(id).catch(() => {});
    setNav("chats");
    // Drafts are per-conversation, so unsent text cannot go to the wrong peer.
    setDraft("");
    try {
      const chat = await api.getConversation(id);
      // A slower response for an earlier click must not replace a later one.
      if (activeIdRef.current !== id) return;
      setActive(chatToContact(chat, convs.find((c) => c.id === id)?.connected));
    } catch { /* ignore */ }
  }

  async function send() {
    const text = draft.trim();
    // Send only to the selected conversation on screen; mid-switch they can
    // differ (audit UI-02).
    const target = active && active.id === activeId ? active.id : null;
    if (!text || !target) return;
    setDraft("");
    // Restore the draft on failure, only into the conversation it was meant for.
    try { await api.sendMessage(target, text); }
    catch (e) {
      if (activeIdRef.current === target) setDraft(text);
      toast(friendlyError(e), "error");
      return;
    }
    refresh();
  }

  async function sendFile(c) {
    try { await api.sendFile(c.id); refresh(); }
    catch (e) { toast(friendlyError(e), "error"); }
  }

  async function doRename(id, title) {
    setRenameTarget(null);
    try { await api.renameChat(id, title); toast("Conversation renamed", "success"); refresh(); }
    catch (e) { toast(friendlyError(e), "error"); }
  }

  async function doDelete(id) {
    setDeleteTarget(null);
    try {
      await api.deleteChat(id);
      if (activeId === id) { setActiveId(null); activeIdRef.current = null; setActive(null); }
      toast("Conversation deleted", "success");
      refresh();
    } catch (e) { toast(friendlyError(e), "error"); }
  }

  return (
    <div className="app-root">
      {/* Skip links, first in tab order, hidden until focused (.skip-link). */}
      <a className="skip-link" href="#main-content">Skip to main content</a>
      {nav === "chats" && activeId && (
        <a className="skip-link" href="#composer-input">Skip to message box</a>
      )}
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
              catch (e) { toast(friendlyError(e), "error"); }
            }}>
            <Icon name={locked ? "lock" : "unlock"} size={16} />
          </button>
          <button className="tb-btn" onClick={() => openCreator()}>
            <Icon name="plus" size={15} /> New connection
          </button>
          <ThemeMenu theme={theme} setTheme={setTheme} />
        </div>
      </header>

      {historyIssue && (
        <HistoryBanner issue={historyIssue} onSetAside={async () => {
          const moved = await api.setAsideHistory();
          setHistoryIssue(null);
          toast(moved ? `Old history kept as ${moved}` : "Started a new history", "info");
        }} />
      )}
      <div className="app-body">
        {/* Labelled, so this landmark is distinguishable from the others. */}
        <nav className="rail" role="navigation" aria-label="Primary">
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
            <main className="col-main" id="main-content" tabIndex={-1}>
              <ChatPane contact={active} draft={draft} setDraft={setDraft} onSend={send}
                onSendFile={sendFile}
                // No conversations at all: show the get-started panel.
                isFirstRun={convs.length === 0}
                onStart={openCreator}
                // Never swallow a failed cancel.
                onCancelTransfer={(id) =>
                  api.cancelTransfer(id).catch((e) => toast(`Could not cancel: ${friendlyError(e)}`, "error"))}
                transfers={transfers.filter((t) =>
                  // Finished files already show as messages: surface only
                  // in-flight work and failures.
                  t.chat_id === activeId && t.status !== "done" && t.status !== "cancelled")}
                onAcceptTransfer={async (t) => {
                  try { await api.acceptTransfer(t.id); refresh(); } catch (e) { toast(friendlyError(e), "error"); }
                }}
                onDeclineTransfer={async (t) => {
                  try { await api.declineTransfer(t.id); refresh(); } catch (e) { toast(friendlyError(e), "error"); }
                }}
                onVerify={async (c) => {
                  // A pending TOFU request opens accept/reject; otherwise show
                  // the read-only fingerprint.
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
          <main className="col-full" id="main-content" tabIndex={-1}><Contacts onConnected={() => { setNav("chats"); refresh(); }} /></main>
        )}
        {nav === "settings" && (
          <main className="col-full" id="main-content" tabIndex={-1}><Settings identity={auth} theme={theme} setTheme={setTheme} onIdentityChanged={refreshAuth} /></main>
        )}
        {nav === "parties" && <main className="col-full" id="main-content" tabIndex={-1}><Parties /></main>}
        {nav === "relays" && <main className="col-full" id="main-content" tabIndex={-1}><Relays onConnected={() => { setNav("chats"); refresh(); }} /></main>}
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
