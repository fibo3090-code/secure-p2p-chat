// Communities (Party servers) — administered, multi-channel, persistent rooms
// served by the `messenger-server` crate, driven live through the Tauri bridge
// (party_* commands) and refreshed on the `party-updated` event.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, Input, PasswordInput } from "./ui.jsx";
import { api, onBridge, fmtTime } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";
import { markRead, computeUnread, pruneTo } from "../lib/partyUnread.js";
import {
  DrivePanel, AuditPanel, ChannelAccessDialog, ROLES, kindIcon,
} from "./PartyAdmin.jsx";

const STATUS_LABEL = {
  connecting: "Connecting…",
  joined: "Joined",
  rejected: "Join rejected",
  disconnected: "Disconnected",
};

// Mirror the server-side caps (`messenger-server` state.rs: MAX_USERNAME_CHARS /
// MAX_CHANNEL_NAME_CHARS) so the UI gives immediate feedback instead of relying on
// a server rejection. The server remains authoritative.
const MAX_USERNAME_CHARS = 32;
const MAX_CHANNEL_NAME_CHARS = 64;

function JoinForm({ onJoined, onCancel, initial }) {
  const [address, setAddress] = useState(initial?.address || "");
  const [username, setUsername] = useState(initial?.username || "");
  const [password, setPassword] = useState("");
  const [saved, setSaved] = useState([]);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  // A server whose identity has never been seen before. Nothing has been sent
  // to it yet — not the username, not the password — until the user confirms
  // the code below.
  const [verify, setVerify] = useState(null);

  useEffect(() => {
    // A failure here is not cosmetic: parties.json holds the pinned fingerprint
    // of every community, and the bridge refuses to join when it cannot be read.
    api.partySaved()
      .then((s) => setSaved(s || []))
      .catch((e) => { setSaved([]); setErr(String(e)); });
  }, []);

  async function joinWith(addr, user, pass, trust = false) {
    setErr("");
    if (!addr.trim()) return setErr("Enter the server address.");
    if (!user.trim()) return setErr("Choose a username.");
    if ([...user.trim()].length > MAX_USERNAME_CHARS)
      return setErr(`Username must be ${MAX_USERNAME_CHARS} characters or fewer.`);
    setBusy(true);
    try {
      const res = await api.partyJoin(addr.trim(), user.trim(), pass, trust);
      if (res?.status === "verify") {
        setVerify({ address: addr.trim(), username: user.trim(), password: pass, ...res });
        return;
      }
      toast(`Connecting to ${addr.trim()}…`, "success");
      setVerify(null);
      onJoined && onJoined();
    } catch (e) { setErr(String(e)); setVerify(null); }
    finally { setBusy(false); }
  }

  const join = () => joinWith(address, username, password);

  if (verify) {
    return (
      <div className="chat-pane chat-empty">
        <div className="chat-empty-inner" style={{ maxWidth: 460 }}>
          <span className="chat-empty-ic"><Icon name="lock" size={28} /></span>
          <div className="chat-empty-h">Verify this community server</div>
          <div className="chat-empty-p">
            You have never joined <code>{verify.address}</code> before. Your username and
            password have <strong>not</strong> been sent yet. Ask the operator to read out
            their code and check it matches:
          </div>
          <div className="verify-sas">{verify.sas}</div>
          <details className="party-verify-adv">
            <summary>Advanced: full fingerprint</summary>
            <code className="vf-fp-code">{verify.fingerprint}</code>
          </details>
          <div className="party-join">
            <Button icon="check" disabled={busy} full
              onClick={() => joinWith(verify.address, verify.username, verify.password, true)}>
              The code matches — join
            </Button>
            <Button variant="ghost" full disabled={busy}
              onClick={() => { setVerify(null); setBusy(false); }}>
              Cancel
            </Button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="chat-pane chat-empty">
      <div className="chat-empty-inner" style={{ maxWidth: 440 }}>
        <span className="chat-empty-ic"><Icon name="users" size={28} /></span>
        <div className="chat-empty-h">Join a community</div>
        <div className="chat-empty-p">
          Communities are administered, multi-channel rooms that keep history, served by the
          <code> messenger-server</code> crate. The first time you join one you will be asked to
          check its code with the operator before anything is sent.
        </div>
        {saved.length > 0 && (
          <div className="party-saved">
            <div className="party-saved-h">Your communities</div>
            {saved.map((p) => (
              // Selecting a saved community fills the form; it does not join.
              // Joining straight from the card sent whatever happened to be in
              // the shared password box — so a password typed for one
              // community went to whichever card was clicked next.
              <button key={p.address} className="party-saved-card" disabled={busy}
                title={`Fill in ${p.username}@${p.address}`}
                onClick={() => { setAddress(p.address); setUsername(p.username); setPassword(""); setErr(""); }}>
                <Avatar name={p.name || p.address} size={28} party />
                <span className="party-saved-txt">
                  <span className="party-saved-name">{p.name || p.address}</span>
                  <span className="party-saved-sub mono">{p.username} · {p.address}</span>
                </span>
                <Icon name="chevronRight" size={14} />
              </button>
            ))}
            <div className="party-saved-hint">
              Pick one to fill the form below, then add its password if it has one and press
              Connect &amp; join.
            </div>
          </div>
        )}
        <div className="party-join">
          <Input value={address} autoFocus placeholder="server address · 192.168.1.20:12345"
            onChange={(e) => setAddress(e.target.value)} />
          <Input value={username} placeholder="username" maxLength={MAX_USERNAME_CHARS}
            onChange={(e) => setUsername(e.target.value)} />
          <PasswordInput value={password} placeholder="server password (optional)"
            onChange={(e) => setPassword(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && join()} />
          <Button icon="users" onClick={join} disabled={busy} full>Connect &amp; join</Button>
          {onCancel && <Button variant="ghost" onClick={onCancel} full>Cancel</Button>}
          {err && <div className="onb-err"><Icon name="alert" size={13} /> {err}</div>}
        </div>
      </div>
    </div>
  );
}

function fmtBytes(n) {
  if (n == null) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

function MessageRow({ m, onDownload, downloading }) {
  const mine = m.from_me;
  const isFile = m.kind === "file";
  return (
    <div className={cx("msg-row", mine ? "is-mine" : "is-them")}>
      <div className="msg-bubble">
        {!mine && <span className="msg-author">{m.sender_name}</span>}
        {isFile ? (
          // A download is a round trip to the community server, so the click
          // must visibly do something — without this the button looked inert
          // and invited repeat clicks that each queued another request.
          <button className="msg-file"
            title={downloading ? "Downloading…" : `Download ${m.text}`}
            onClick={() => onDownload(m)} disabled={!m.hash || downloading}>
            <Icon name="file" size={14} />
            <span className="msg-file-name">{m.text}</span>
            {m.size != null && <span className="msg-file-size">{fmtBytes(m.size)}</span>}
            <Icon name={downloading ? "clock" : "download"} size={14} />
          </button>
        ) : (
          <span className="msg-text">{m.text}</span>
        )}
        <span className="msg-time">{fmtTime(m.timestamp)}</span>
      </div>
    </div>
  );
}

export function Parties() {
  const [servers, setServers] = useState([]);
  const [sid, setSid] = useState(null);      // active server
  const [cid, setCid] = useState(null);      // active channel
  const [dm, setDm] = useState(null);        // active DM peer (member id) — takes precedence
  const [msgs, setMsgs] = useState([]);
  const [draft, setDraft] = useState("");
  const [newChannel, setNewChannel] = useState("");
  const [adding, setAdding] = useState(false);       // show the join form to add another community
  const [prefill, setPrefill] = useState(null);      // prefill for the join form (rejoin flows)
  const [confirmLeave, setConfirmLeave] = useState(false); // two-click leave confirmation
  const [unread, setUnread] = useState({});          // thread key -> unread count
  const [shown, setShown] = useState(150);           // message window (see Messages.jsx)
  const [downloading, setDownloading] = useState({}); // content hash -> in-flight download
  const [view, setView] = useState("chat");          // chat | drive | audit
  const [accessFor, setAccessFor] = useState(null);  // channel being edited, or "new"
  const [confirmDelCh, setConfirmDelCh] = useState(null); // channel id armed for deletion
  const scrollRef = useRef(null);
  useEffect(() => setShown(150), [sid, cid, dm]);
  // Leaving a community must not strand you on its Drive.
  useEffect(() => { setView("chat"); setConfirmDelCh(null); }, [sid]);

  const server = useMemo(() => servers.find((s) => s.id === sid) || null, [servers, sid]);

  const loadServers = useCallback(async () => {
    try {
      const list = await api.partyList();
      // Unread bookkeeping: the thread on screen is always read; badge the rest.
      const srv = list.find((s) => s.id === sid);
      if (srv) {
        if (dm) markRead(sid, `dm-${dm}`, srv.members.find((m) => m.id === dm)?.dm_messages);
        else if (cid) markRead(sid, cid, srv.channels.find((c) => c.id === cid)?.messages);
      }
      // Forget marks for communities the user has left, so the persisted store
      // does not grow without bound as servers come and go.
      pruneTo(list);
      setUnread(computeUnread(list).byKey);
      setServers(list);
    } catch { /* ignore */ }
  }, [sid, cid, dm]);

  const loadMsgs = useCallback(async () => {
    if (!sid) { setMsgs([]); return; }
    try {
      if (dm) setMsgs(await api.partyDmHistory(sid, dm));
      else if (cid) setMsgs(await api.partyHistory(sid, cid));
      else setMsgs([]);
    } catch { /* ignore */ }
  }, [sid, cid, dm]);

  // Opening the Drive or the log asks the server for it; the answer arrives
  // asynchronously and lands in the next `party-updated` tick.
  useEffect(() => {
    if (!sid || view === "chat") return;
    const ask = view === "drive" ? api.partyRefreshFiles : api.partyRefreshAudit;
    ask(sid).catch(() => {});
  }, [sid, view]);

  // Surface a completed governance action once, then clear it so it does not
  // re-toast on every poll.
  useEffect(() => {
    if (!server?.last_notice) return;
    toast(server.last_notice, "success");
    api.partyClearNotice(server.id).catch(() => {});
  }, [server?.last_notice, server?.id]);

  // Initial load + live refresh on every poll tick.
  useEffect(() => { loadServers(); }, [loadServers]);
  useEffect(() => {
    const sub = onBridge("party-updated", () => { loadServers(); loadMsgs(); });
    return () => { sub.then((f) => f && f()).catch(() => {}); };
  }, [loadServers, loadMsgs]);
  useEffect(() => { loadMsgs(); }, [loadMsgs]);

  // Keep the active server / channel selection valid as the directory changes.
  useEffect(() => {
    if (!servers.length) { setSid(null); return; }
    if (!sid || !servers.some((s) => s.id === sid)) setSid(servers[0].id);
  }, [servers, sid]);
  useEffect(() => {
    if (!server || dm) return;
    if (!cid || !server.channels.some((c) => c.id === cid)) {
      setCid(server.channels[0]?.id ?? null);
    }
  }, [server, cid, dm]);

  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [msgs.length, cid, dm]);

  if (!servers.length || adding) {
    return (
      <JoinForm initial={prefill}
        onJoined={() => { setAdding(false); setPrefill(null); loadServers(); }}
        onCancel={servers.length ? () => { setAdding(false); setPrefill(null); } : null} />
    );
  }

  const peer = dm ? server?.members.find((m) => m.id === dm) : null;
  const channel = !dm ? server?.channels.find((c) => c.id === cid) : null;
  const threadName = dm ? `✉ ${peer?.username || "member"}` : `# ${channel?.name || ""}`;
  const connected = server?.status === "joined";
  const isAdmin = server?.my_role === "admin" || server?.my_role === "owner";
  // A guest is read-only everywhere; a channel's kind can also refuse an
  // ordinary member. The server decides either way — this only means the
  // composer explains itself instead of accepting a message that gets refused.
  const readOnly = server?.my_role === "guest";
  const canSend = connected && !readOnly && (dm ? true : channel?.can_post !== false);
  const composerHint = !connected
    ? "Not connected"
    : readOnly
      ? "Your role on this server is read-only"
      : !dm && channel?.can_post === false
        ? channel.kind === "announce"
          ? "Only admins can post to an announcement channel"
          : "This channel is locked"
        : `Message ${threadName}`;

  async function send() {
    const text = draft.trim();
    if (!text || !server) return;
    setDraft("");
    try {
      if (dm) await api.partySendDm(server.id, dm, text);
      else if (cid) await api.partyPost(server.id, cid, text);
      loadMsgs();
    } catch (e) { setDraft(text); toast(String(e), "error"); }
  }

  // Share a file into the active channel or DM. The native picker opens in the
  // bridge; a cancelled dialog is a no-op.
  async function sendFile() {
    if (!server || !connected) return;
    try {
      if (dm) await api.partySendFileDm(server.id, dm);
      else if (cid) await api.partySendFile(server.id, cid);
      else return;
      loadMsgs();
    } catch (e) { toast(String(e), "error"); }
  }

  // Download a file message's bytes and save them via the native dialog.
  // Tracked per content hash so the card shows it is working: the request can
  // take as long as the server takes to answer, and a click with no feedback
  // is indistinguishable from a broken button.
  async function downloadFile(m) {
    if (!server || !m.hash || downloading[m.hash]) return;
    setDownloading((d) => ({ ...d, [m.hash]: true }));
    try { await api.partyDownloadFile(server.id, m.hash, m.text); }
    catch (e) { toast(String(e), "error"); }
    finally {
      setDownloading((d) => {
        const next = { ...d };
        delete next[m.hash];
        return next;
      });
    }
  }

  async function createChannel() {
    const name = newChannel.trim();
    if (!name || !server) return;
    if ([...name].length > MAX_CHANNEL_NAME_CHARS) {
      toast(`Channel name must be ${MAX_CHANNEL_NAME_CHARS} characters or fewer.`, "error");
      return;
    }
    setNewChannel("");
    try { await api.partyCreateChannel(server.id, name); loadServers(); }
    catch (e) { toast(String(e), "error"); }
  }

  // Create a channel of a chosen kind, or change an existing one's access.
  async function submitAccess({ name, kind, members }) {
    if (!server) return;
    try {
      if (accessFor === "new") {
        await api.partyCreateChannelKind(server.id, name, kind, members);
      } else {
        await api.partySetChannelAccess(server.id, accessFor.id, kind, members);
      }
      loadServers();
    } catch (e) { toast(String(e), "error"); }
  }

  // Delete a channel and its history (two-click, admins only).
  async function deleteChannel(channel) {
    if (!server) return;
    if (confirmDelCh !== channel.id) { setConfirmDelCh(channel.id); return; }
    setConfirmDelCh(null);
    try {
      await api.partyDeleteChannel(server.id, channel.id);
      if (cid === channel.id) setCid(null);
      loadServers();
    } catch (e) { toast(String(e), "error"); }
  }

  async function setRole(member, role) {
    if (!server) return;
    try { await api.partySetRole(server.id, member.id, role); loadServers(); }
    catch (e) { toast(String(e), "error"); }
  }

  async function deleteFile(f) {
    if (!server) return;
    try {
      await api.partyDeleteFile(server.id, f.hash, f.location);
      await api.partyRefreshFiles(server.id);
      loadServers();
    } catch (e) { toast(String(e), "error"); }
  }

  async function dismissError() {
    if (!server) return;
    try { await api.partyClearError(server.id); loadServers(); } catch { /* ignore */ }
  }

  // Leave (two-click confirm): drops the connection and forgets the community
  // locally. The server keeps the membership, so rejoining later resumes it.
  async function leave() {
    if (!server) return;
    // The first click arms it; the label switches to spell out that the pinned
    // fingerprint goes with it.
    if (!confirmLeave) { setConfirmLeave(true); return; }
    setConfirmLeave(false);
    try {
      await api.partyLeave(server.id);
      toast(`Left ${server.name || server.address}.`, "success");
      setDm(null); setCid(null);
      loadServers();
    } catch (e) { toast(String(e), "error"); }
  }

  // Rejoin a dropped/rejected community: open the join form prefilled with the
  // stored address + username (a password can be typed there if needed). A
  // successful rejoin replaces this entry (deduped by address in the bridge).
  function rejoin() {
    if (!server) return;
    setPrefill({ address: server.address, username: server.username || "" });
    setAdding(true);
  }

  // Remove a dead entry without rejoining. Two-click like `leave`: this also
  // discards the community's pinned fingerprint, so rejoining afterwards is an
  // unverified first contact again.
  async function removeServer() {
    if (!server) return;
    if (!confirmLeave) { setConfirmLeave(true); return; }
    setConfirmLeave(false);
    try {
      await api.partyLeave(server.id);
      setDm(null); setCid(null);
      loadServers();
    } catch (e) { toast(String(e), "error"); }
  }

  return (
    <div className="party-layout">
      <div className="party-servers">
        {servers.map((s) => {
          const srvUnread = Object.entries(unread)
            .reduce((n, [k, v]) => (k.startsWith(`${s.id}|`) ? n + v : n), 0);
          return (
            <button key={s.id} className={cx("party-srv-tab", s.id === sid && "is-active")}
              onClick={() => { setSid(s.id); setDm(null); setConfirmLeave(false); }}>
              <Icon name="users" size={14} /> {s.name || s.address}
              {srvUnread > 0 && <span className="party-unread">{srvUnread}</span>}
            </button>
          );
        })}
        <button className="party-srv-tab party-srv-add" title="Join another community"
          onClick={() => { setPrefill(null); setAdding(true); }}>
          <Icon name="plus" size={14} />
        </button>
      </div>

      <aside className="party-side">
        <div className="party-side-h">Channels</div>
        <div className="party-list">
          {server?.channels.map((c) => (
            <div key={c.id} className="party-row">
              <button className={cx("party-item", view === "chat" && !dm && c.id === cid && "is-active")}
                title={c.kind === "public" ? `#${c.name}` : `#${c.name} — ${c.kind}`}
                onClick={() => {
                  setView("chat"); setDm(null); setCid(c.id);
                  markRead(server.id, c.id, c.messages);
                }}>
                <span className="party-hash"><Icon name={kindIcon(c.kind)} size={12} /></span> {c.name}
                {unread[`${server.id}|${c.id}`] > 0 && (
                  <span className="party-unread">{unread[`${server.id}|${c.id}`]}</span>
                )}
              </button>
              {isAdmin && (
                <span className="party-rowacts">
                  <button title={`Change who can use #${c.name}`} onClick={() => setAccessFor(c)}>
                    <Icon name="settings" size={13} />
                  </button>
                  <button
                    className={cx(confirmDelCh === c.id && "is-confirm")}
                    title={confirmDelCh === c.id
                      ? `Click again to delete #${c.name} and all of its history.`
                      : `Delete #${c.name}`}
                    onBlur={() => setConfirmDelCh(null)}
                    onClick={() => deleteChannel(c)}>
                    <Icon name="trash" size={13} />
                  </button>
                </span>
              )}
            </div>
          ))}
        </div>
        <div className="party-newch">
          <input placeholder="new channel" value={newChannel} maxLength={MAX_CHANNEL_NAME_CHARS}
            onChange={(e) => setNewChannel(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && createChannel()} />
          <button title="Create channel" onClick={createChannel}><Icon name="plus" size={15} /></button>
          {isAdmin && (
            <button title="New channel with a specific kind" onClick={() => setAccessFor("new")}>
              <Icon name="settings" size={15} />
            </button>
          )}
        </div>

        <div className="party-side-h">Members ({server?.members.length || 0})</div>
        <div className="party-list">
          {server?.members.map((m) => (
            <div key={m.id} className="party-row">
              <button disabled={m.is_me}
                className={cx("party-item", "party-member", view === "chat" && dm === m.id && "is-active")}
                onClick={() => {
                  if (m.is_me) return;
                  setView("chat"); setDm(m.id);
                  markRead(server.id, `dm-${m.id}`, m.dm_messages);
                }}
                title={m.is_me ? "You" : `Direct message ${m.username}`}>
                <span className={cx("party-dot", m.online ? "is-online" : "is-offline")} />
                {m.is_me ? <>{m.username} <span className="party-you">you</span></> : <><Icon name="user" size={12} /> {m.username}</>}
                {m.role !== "member" && (
                  <span className={cx("party-role", "role-" + m.role)}>{m.role}</span>
                )}
                {!m.is_me && unread[`${server.id}|dm-${m.id}`] > 0 && (
                  <span className="party-unread">{unread[`${server.id}|dm-${m.id}`]}</span>
                )}
              </button>
              {/* The owner's role is fixed, and you cannot change your own. */}
              {isAdmin && !m.is_me && m.role !== "owner" && (
                <select className="party-rolesel" value={m.role}
                  title={`Role for ${m.username}`}
                  onChange={(e) => setRole(m, e.target.value)}>
                  {ROLES.map((r) => <option key={r} value={r}>{r}</option>)}
                </select>
              )}
            </div>
          ))}
        </div>
      </aside>

      <main className="party-main">
        <header className="party-head">
          <div className="party-head-l">
            <Avatar name={server?.name || server?.address} size={34} party />
            <div>
              <div className="party-head-name">{threadName}</div>
              <div className="party-head-sub mono">
                {server?.name || server?.address}
                <span className="chat-dot">·</span>
                <span className={cx("party-status", "st-" + (server?.status || ""))}>
                  {STATUS_LABEL[server?.status] || server?.status}
                  {server?.status_detail ? `: ${server.status_detail}` : ""}
                </span>
              </div>
            </div>
          </div>
          <div className="party-head-r">
            <div className="party-views" role="tablist" aria-label="Community view">
              <button role="tab" aria-selected={view === "chat"}
                className={cx("party-view", view === "chat" && "is-active")}
                onClick={() => setView("chat")} title="Conversation">
                <Icon name="message" size={14} />
              </button>
              <button role="tab" aria-selected={view === "drive"}
                className={cx("party-view", view === "drive" && "is-active")}
                onClick={() => setView("drive")} title="Shared files">
                <Icon name="folder" size={14} />
              </button>
              {isAdmin && (
                <button role="tab" aria-selected={view === "audit"}
                  className={cx("party-view", view === "audit" && "is-active")}
                  onClick={() => setView("audit")} title="Activity log">
                  <Icon name="clock" size={14} />
                </button>
              )}
            </div>
            <div className="party-fp" title="Verify this out of band">
              <Icon name="fingerprint" size={13} />
              <code>{(server?.fingerprint || "").slice(0, 24)}…</code>
            </div>
            <button className={cx("party-leave", confirmLeave && "is-confirm")}
              title={confirmLeave
                ? "Click again to leave. This also forgets the server's verified fingerprint, so rejoining means checking its code again."
                : "Leave this community"}
              onClick={leave} onBlur={() => setConfirmLeave(false)}>
              <Icon name="x" size={14} /> {confirmLeave ? "Leave?" : "Leave"}
            </button>
          </div>
        </header>

        {(server?.status === "disconnected" || server?.status === "rejected") && (
          <div className="party-banner">
            <Icon name="alert" size={14} />
            <span>
              {server.status === "rejected"
                ? `Join rejected${server.status_detail ? `: ${server.status_detail}` : ""}.`
                : "Connection to this community was lost."}
            </span>
            <Button size="sm" icon="users" onClick={rejoin}>Rejoin</Button>
            <Button size="sm" variant="ghost" onClick={removeServer}>Remove</Button>
          </div>
        )}

        {server?.last_error && (
          <div className="party-error">
            <Icon name="alert" size={14} /> <span>{server.last_error}</span>
            <button onClick={dismissError} title="Dismiss"><Icon name="x" size={14} /></button>
          </div>
        )}

        {view === "drive" && (
          <DrivePanel server={server} downloading={downloading}
            onDownload={downloadFile} onDelete={deleteFile} />
        )}
        {view === "audit" && <AuditPanel server={server} />}

        {view === "chat" && (
          <>
            <div className="chat-scroll" ref={scrollRef}>
              <div className="chat-thread">
                {msgs.length === 0 && <div className="conv-empty">No messages yet.</div>}
                {msgs.length > shown && (
                  <button className="thread-more" onClick={() => setShown((s) => s + 150)}>
                    Show earlier messages ({msgs.length - shown} more)
                  </button>
                )}
                {msgs.slice(-shown).map((m, i) => (
                  <MessageRow key={i} m={m} onDownload={downloadFile}
                    downloading={!!(m.hash && downloading[m.hash])} />
                ))}
              </div>
            </div>

            <div className="composer">
              <button className="composer-clip" onClick={sendFile} title="Share a file"
                disabled={!canSend}>
                <Icon name="paperclip" size={18} />
              </button>
              <textarea className="composer-input" rows={1}
                placeholder={composerHint}
                disabled={!canSend}
                value={draft} onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); send(); } }} />
              <button className={cx("composer-send", draft.trim() && "is-ready")} onClick={send}
                title="Send" disabled={!canSend}>
                <Icon name="send" size={18} />
              </button>
            </div>
          </>
        )}
      </main>

      {accessFor && (
        <ChannelAccessDialog server={server}
          channel={accessFor === "new" ? null : accessFor}
          onClose={() => setAccessFor(null)}
          onSubmit={submitAccess} />
      )}
    </div>
  );
}
