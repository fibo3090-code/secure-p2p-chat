// Communities (Party servers) — administered, multi-channel, persistent rooms
// served by the `messenger-server` crate, driven live through the Tauri bridge
// (party_* commands) and refreshed on the `party-updated` event.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, Input, PasswordInput } from "./ui.jsx";
import { api, onBridge, fmtTime } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";

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

  useEffect(() => {
    api.partySaved().then((s) => setSaved(s || [])).catch(() => {});
  }, []);

  async function joinWith(addr, user, pass) {
    setErr("");
    if (!addr.trim()) return setErr("Enter the server address.");
    if (!user.trim()) return setErr("Choose a username.");
    if ([...user.trim()].length > MAX_USERNAME_CHARS)
      return setErr(`Username must be ${MAX_USERNAME_CHARS} characters or fewer.`);
    setBusy(true);
    try {
      await api.partyJoin(addr.trim(), user.trim(), pass);
      toast(`Joining ${addr.trim()}…`, "success");
      onJoined && onJoined();
    } catch (e) { setErr(String(e)); }
    finally { setBusy(false); }
  }

  const join = () => joinWith(address, username, password);

  return (
    <div className="chat-pane chat-empty">
      <div className="chat-empty-inner" style={{ maxWidth: 440 }}>
        <span className="chat-empty-ic"><Icon name="users" size={28} /></span>
        <div className="chat-empty-h">Join a community</div>
        <div className="chat-empty-p">
          Communities are administered, multi-channel rooms that keep history, served by the
          <code> messenger-server</code> crate. Verify the server's fingerprint out of band after joining.
        </div>
        {saved.length > 0 && (
          <div className="party-saved">
            <div className="party-saved-h">Your communities</div>
            {saved.map((p) => (
              <button key={p.address} className="party-saved-card" disabled={busy}
                title={`Rejoin as ${p.username}`}
                onClick={() => { setAddress(p.address); setUsername(p.username); joinWith(p.address, p.username, password); }}>
                <Avatar name={p.name || p.address} size={28} party />
                <span className="party-saved-txt">
                  <span className="party-saved-name">{p.name || p.address}</span>
                  <span className="party-saved-sub mono">{p.username} · {p.address}</span>
                </span>
                <Icon name="chevronRight" size={14} />
              </button>
            ))}
            <div className="party-saved-hint">
              Joining a password-protected community? Type the password below first, then click it.
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

function MessageRow({ m, onDownload }) {
  const mine = m.from_me;
  const isFile = m.kind === "file";
  return (
    <div className={cx("msg-row", mine ? "is-mine" : "is-them")}>
      <div className="msg-bubble">
        {!mine && <span className="msg-author">{m.sender_name}</span>}
        {isFile ? (
          <button className="msg-file" title={`Download ${m.text}`}
            onClick={() => onDownload(m)} disabled={!m.hash}>
            <Icon name="file" size={14} />
            <span className="msg-file-name">{m.text}</span>
            {m.size != null && <span className="msg-file-size">{fmtBytes(m.size)}</span>}
            <Icon name="download" size={14} />
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
  const scrollRef = useRef(null);

  const server = useMemo(() => servers.find((s) => s.id === sid) || null, [servers, sid]);

  const loadServers = useCallback(async () => {
    try { setServers(await api.partyList()); } catch { /* ignore */ }
  }, []);

  const loadMsgs = useCallback(async () => {
    if (!sid) { setMsgs([]); return; }
    try {
      if (dm) setMsgs(await api.partyDmHistory(sid, dm));
      else if (cid) setMsgs(await api.partyHistory(sid, cid));
      else setMsgs([]);
    } catch { /* ignore */ }
  }, [sid, cid, dm]);

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
  const threadName = dm ? `✉ ${peer?.username || "member"}` : `# ${server?.channels.find((c) => c.id === cid)?.name || ""}`;
  const connected = server?.status === "joined";

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
  async function downloadFile(m) {
    if (!server || !m.hash) return;
    try { await api.partyDownloadFile(server.id, m.hash, m.text); }
    catch (e) { toast(String(e), "error"); }
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

  async function dismissError() {
    if (!server) return;
    try { await api.partyClearError(server.id); loadServers(); } catch { /* ignore */ }
  }

  // Leave (two-click confirm): drops the connection and forgets the community
  // locally. The server keeps the membership, so rejoining later resumes it.
  async function leave() {
    if (!server) return;
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

  // Remove a dead entry without rejoining.
  async function removeServer() {
    if (!server) return;
    try {
      await api.partyLeave(server.id);
      setDm(null); setCid(null);
      loadServers();
    } catch (e) { toast(String(e), "error"); }
  }

  return (
    <div className="party-layout">
      <div className="party-servers">
        {servers.map((s) => (
          <button key={s.id} className={cx("party-srv-tab", s.id === sid && "is-active")}
            onClick={() => { setSid(s.id); setDm(null); setConfirmLeave(false); }}>
            <Icon name="users" size={14} /> {s.name || s.address}
          </button>
        ))}
        <button className="party-srv-tab party-srv-add" title="Join another community"
          onClick={() => { setPrefill(null); setAdding(true); }}>
          <Icon name="plus" size={14} />
        </button>
      </div>

      <aside className="party-side">
        <div className="party-side-h">Channels</div>
        <div className="party-list">
          {server?.channels.map((c) => (
            <button key={c.id} className={cx("party-item", !dm && c.id === cid && "is-active")}
              onClick={() => { setDm(null); setCid(c.id); }}>
              <span className="party-hash">#</span> {c.name}
            </button>
          ))}
        </div>
        <div className="party-newch">
          <input placeholder="new channel" value={newChannel} maxLength={MAX_CHANNEL_NAME_CHARS}
            onChange={(e) => setNewChannel(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && createChannel()} />
          <button title="Create channel" onClick={createChannel}><Icon name="plus" size={15} /></button>
        </div>

        <div className="party-side-h">Members ({server?.members.length || 0})</div>
        <div className="party-list">
          {server?.members.map((m) => (
            <button key={m.id} disabled={m.is_me}
              className={cx("party-item", "party-member", dm === m.id && "is-active")}
              onClick={() => !m.is_me && setDm(m.id)}
              title={m.is_me ? "You" : `Direct message ${m.username}`}>
              <span className={cx("party-dot", m.online ? "is-online" : "is-offline")} />
              {m.is_me ? <>{m.username} <span className="party-you">you</span></> : <><Icon name="user" size={12} /> {m.username}</>}
            </button>
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
            <div className="party-fp" title="Verify this out of band">
              <Icon name="fingerprint" size={13} />
              <code>{(server?.fingerprint || "").slice(0, 24)}…</code>
            </div>
            <button className={cx("party-leave", confirmLeave && "is-confirm")}
              title={confirmLeave ? "Click again to confirm leaving" : "Leave this community"}
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

        <div className="chat-scroll" ref={scrollRef}>
          <div className="chat-thread">
            {msgs.length === 0 && <div className="conv-empty">No messages yet.</div>}
            {msgs.map((m, i) => <MessageRow key={i} m={m} onDownload={downloadFile} />)}
          </div>
        </div>

        <div className="composer">
          <button className="composer-clip" onClick={sendFile} title="Share a file" disabled={!connected}>
            <Icon name="paperclip" size={18} />
          </button>
          <textarea className="composer-input" rows={1}
            placeholder={connected ? `Message ${threadName}` : "Not connected"}
            disabled={!connected}
            value={draft} onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); send(); } }} />
          <button className={cx("composer-send", draft.trim() && "is-ready")} onClick={send} title="Send" disabled={!connected}>
            <Icon name="send" size={18} />
          </button>
        </div>
      </main>
    </div>
  );
}
