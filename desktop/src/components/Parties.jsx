// Communities (Party servers) — administered, multi-channel, persistent rooms
// served by the `messenger-server` crate, driven live through the Tauri bridge
// (party_* commands) and refreshed on the `party-updated` event.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "../lib/Icon.jsx";
import { cx, Avatar, Button, Input, PasswordInput, Page } from "./ui.jsx";
import { api, onBridge, fmtTime, friendlyError } from "../lib/bridge.js";
import { toast } from "../lib/toast.js";
import { SafetyGrid } from "./SafetyGrid.jsx";
import { markRead, computeUnread, pruneTo } from "../lib/partyUnread.js";
import {
  DrivePanel, AuditPanel, ChannelAccessDialog, FilePermissionsDialog, ShareFileDialog,
  ROLES, kindIcon,
} from "./PartyAdmin.jsx";

// Roles this viewer may actually assign to `member`, mirroring the server's rule
// (`Role::may_assign` plus the "not at or above your own" check). Rendering the
// full list meant an admin was offered `admin` for a peer admin — pre-selected,
// even — and every attempt came back refused.
function assignableRoles(myRole, member) {
  if (member.is_me || member.role === "owner") return [];
  const rank = { guest: 0, member: 1, admin: 2, owner: 3 };
  const mine = rank[myRole] ?? 0;
  if (mine < rank.admin) return [];
  // You cannot touch somebody at or above your own level.
  if ((rank[member.role] ?? 0) >= mine) return [];
  return ROLES.filter((r) => rank[r] < mine);
}

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

/// A 64-hex fingerprint in groups of eight, for reading aloud and comparing.
function groupFingerprint(fp) {
  return (fp || "").match(/.{1,8}/g)?.join(" ") ?? "";
}

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
      .catch((e) => { setSaved([]); setErr(friendlyError(e)); });
  }, []);

  async function joinWith(addr, user, pass, fingerprint = null) {
    setErr("");
    if (!addr.trim()) return setErr("Enter the server address.");
    if (!user.trim()) return setErr("Choose a username.");
    if ([...user.trim()].length > MAX_USERNAME_CHARS)
      return setErr(`Username must be ${MAX_USERNAME_CHARS} characters or fewer.`);
    setBusy(true);
    try {
      const res = await api.partyJoin(addr.trim(), user.trim(), pass, fingerprint);
      if (res?.status === "verify") {
        setVerify({ address: addr.trim(), username: user.trim(), password: pass, ...res });
        return;
      }
      toast(`Connecting to ${addr.trim()}…`, "success");
      setVerify(null);
      onJoined && onJoined();
    } catch (e) { setErr(friendlyError(e)); setVerify(null); }
    finally { setBusy(false); }
  }

  const join = () => joinWith(address, username, password);
  const pickedSaved = saved.find((p) => p.address === address.trim() && p.username === username.trim());
  // With saved communities, the blank form waits behind "Join another
  // community" — picking one of yours is the common path.
  const [another, setAnother] = useState(false);
  // A known community only needs its password; the address and username are
  // already right, so they stay folded unless the user asks to edit them.
  const [editDetails, setEditDetails] = useState(false);
  const showForm = saved.length === 0 || !!pickedSaved || another;
  const passRef = useRef(null);
  // Field-level errors sit on the field they are about, not at the bottom of
  // the form under the password box.
  const errField = err === "Enter the server address." ? "addr"
    : /^(Choose a username|Username must)/.test(err) ? "user" : null;
  const fieldErr = (id) => errField === id && (
    <div className="onb-err" id={`party-${id}-err`}><Icon name="alert" size={13} /> {err}</div>
  );

  // Said once, as a calm callout rather than a bold clause buried in the middle
  // of a paragraph: what a community is, and the one thing it is not.
  const notE2E = (
    <div className="callout is-warn">
      <Icon name="shieldAlert" size={16} />
      <p>
        <strong>Communities are not end-to-end encrypted.</strong> Messages, DMs and
        files are stored on the server, readable by whoever runs it. For private
        conversations, use a direct chat.
      </p>
    </div>
  );

  if (verify) {
    return (
      <Page title="Communities" sub="Check the server before anything is sent to it.">
        <section className="card is-focus">
          <div className="card-ic"><Icon name="fingerprint" size={22} /></div>
          <h2 className="card-title">Verify this community server</h2>
          <p className="card-lead">
            You have never joined <code>{verify.address}</code> before. Your username and
            password have <strong>not</strong> been sent yet. Check that this fingerprint
            matches the one the operator gave you — their server prints it every time it
            starts:
          </p>
          <div className="party-verify-fp-row">
            <SafetyGrid fingerprint={verify.fingerprint} n={8} cell={18} />
            <div className="party-verify-fp-side">
              <code className="vf-fp-code party-verify-fp">{groupFingerprint(verify.fingerprint)}</code>
              <Button size="sm" variant="ghost" icon="copy" onClick={() => navigator.clipboard?.writeText(verify.fingerprint)
                .then(() => toast("Fingerprint copied", "success"))
                .catch(() => toast("Could not copy", "error"))}>Copy</Button>
            </div>
          </div>
          {notE2E}
          <div className="card-actions">
            <Button variant="ghost" disabled={busy}
              onClick={() => { setVerify(null); setBusy(false); }}>
              Cancel
            </Button>
            <Button icon="check" disabled={busy}
              onClick={() => joinWith(verify.address, verify.username, verify.password, verify.fingerprint)}>
              The fingerprint matches — join
            </Button>
          </div>
        </section>
      </Page>
    );
  }

  return (
    <Page title="Communities"
      sub="Shared, multi-channel rooms hosted on a server someone runs."
      actions={onCancel && <Button variant="ghost" icon="arrowLeft" onClick={onCancel}>Back</Button>}>
      {notE2E}
      {saved.length > 0 && (
        <section>
          <h2 className="section-h">Your communities</h2>
          <div className="list-card">
            {saved.map((p) => {
              const picked = p.address === address.trim() && p.username === username.trim();
              return (
                // Selecting a saved community fills the form; it does not join.
                // Joining straight from the card sent whatever happened to be in
                // the shared password box — so a password typed for one
                // community went to whichever card was clicked next.
                <button key={p.address} className={cx("list-row", picked && "is-picked")} disabled={busy}
                  title={`Fill in ${p.username}@${p.address}`} aria-pressed={picked}
                  onClick={() => {
                    setAddress(p.address); setUsername(p.username); setPassword(""); setErr(""); setEditDetails(false);
                    // Next step is the password (if it has one), then Join.
                    setTimeout(() => passRef.current?.focus(), 0);
                  }}>
                  <Avatar name={p.name || p.address} size={36} party />
                  <span className="list-row-main">
                    <span className="list-row-name">{p.name || p.address}</span>
                    <span className="list-row-sub">Username: {p.username} · {p.address} · <Icon name="lock" size={11} /> Server key pinned</span>
                  </span>
                  <span className={cx("list-row-use", picked && "is-on")}>{picked ? <><Icon name="check" size={14} /> Selected</> : "Rejoin"}</span>
                </button>
              );
            })}
          </div>
        </section>
      )}
      {!showForm && (
        <div>
          <Button variant="ghost" icon="plus" onClick={() => setAnother(true)}>Join another community</Button>
        </div>
      )}
      {showForm && <form className="card" onSubmit={(e) => { e.preventDefault(); void join(); }}>
        <h2 className="card-title">{pickedSaved ? `Rejoin ${pickedSaved.name || pickedSaved.address}` : "Join a community"}</h2>
        <p className="card-lead">
          {pickedSaved
            ? "Add its password if it has one, then join."
            : "Ask the operator for the server address and, if it has one, its password. The first time, you'll check its fingerprint before anything is sent."}
        </p>
        {pickedSaved && !editDetails ? (
          <p className="field-hint">
            As <strong>{pickedSaved.username}</strong> on {pickedSaved.address}
            {" · "}<button type="button" className="chat-empty-link" onClick={() => setEditDetails(true)}>Edit details</button>
          </p>
        ) : (
                <div className="field-grid">
          <div className="field">
            <label className="field-label" htmlFor="party-addr">Server address</label>
            <Input id="party-addr" value={address} autoFocus placeholder="192.168.1.20:12345"
              aria-invalid={errField === "addr" || undefined}
              aria-describedby={errField === "addr" ? "party-addr-err" : undefined}
              onChange={(e) => setAddress(e.target.value)} />
            {fieldErr("addr")}
          </div>
          <div className="field">
            <label className="field-label" htmlFor="party-user">Username</label>
            <Input id="party-user" value={username} placeholder="How others will see you" maxLength={MAX_USERNAME_CHARS}
              aria-invalid={errField === "user" || undefined}
              aria-describedby={errField === "user" ? "party-user-err" : undefined}
              onChange={(e) => setUsername(e.target.value)} />
            {fieldErr("user")}
          </div>
        </div>
        )}
        <div className="field">
          <label className="field-label" htmlFor="party-pass">Server password <span className="field-opt">optional</span></label>
          <PasswordInput id="party-pass" ref={passRef} value={password} placeholder="Leave empty if the server has none"
            autoComplete="off"
            onChange={(e) => setPassword(e.target.value)} />
        </div>
        {err && !errField && <div className="onb-err"><Icon name="alert" size={13} /> {err}</div>}
        <div className="card-actions">
          <Button icon="users" type="submit" disabled={busy}>Connect &amp; join</Button>
        </div>
      </form>}
    </Page>
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
  // One draft per thread (UI-03). A single shared draft followed the user from
  // a private DM into a public channel, where Enter posted it to the whole
  // community — the P2P composer clears on switch for exactly this reason.
  const [drafts, setDrafts] = useState({});
  const thread = `${sid}|${dm ? `dm:${dm}` : cid}`;
  const draft = drafts[thread] || "";
  const setDraftIn = (key, text) => setDrafts((d) => ({ ...d, [key]: text }));
  const setDraft = (text) => setDraftIn(thread, text);
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
  const [permsFor, setPermsFor] = useState(null);    // file whose grants are being edited
  const [shareFor, setShareFor] = useState(null);    // file being re-shared elsewhere
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
        if (dm) markRead(srv, `dm-${dm}`, srv.members.find((m) => m.id === dm)?.dm_messages);
        else if (cid) markRead(srv, cid, srv.channels.find((c) => c.id === cid)?.messages);
      }
      // Forget marks for communities the user has left — judged against the
      // saved list, which still names a community that has not rejoined yet.
      // If it cannot be read, keep everything: losing marks is the worse error.
      const saved = await api.partySaved().catch(() => null);
      if (Array.isArray(saved)) pruneTo([...list, ...saved]);
      setUnread(computeUnread(list).byKey);
      setServers(list);
    } catch { /* ignore */ }
  }, [sid, cid, dm]);

  // The thread on screen, read when a history response lands. Every selection
  // change and every `party-updated` tick starts a fetch, and one for the
  // thread just left can resolve last; writing it would show that thread's
  // messages — a private channel's, say — under this one's name (UI-04).
  const threadRef = useRef(null);
  threadRef.current = thread;
  const loadMsgs = useCallback(async () => {
    if (!sid) { setMsgs([]); return; }
    const wanted = `${sid}|${dm ? `dm:${dm}` : cid}`;
    try {
      let next = [];
      if (dm) next = await api.partyDmHistory(sid, dm);
      else if (cid) next = await api.partyHistory(sid, cid);
      if (threadRef.current === wanted) setMsgs(next);
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
    // `view` matters: the chat pane unmounts for the Drive and log, so coming
    // back mounts a fresh node at scrollTop 0 and the thread looks scrolled up.
  }, [msgs.length, cid, dm, view]);

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
    // A refusal can arrive after the user has moved on: the text goes back to
    // the thread it was written for, not whichever one is open by then.
    const from = thread;
    setDraftIn(from, "");
    try {
      if (dm) await api.partySendDm(server.id, dm, text);
      else if (cid) await api.partyPost(server.id, cid, text);
      loadMsgs();
    } catch (e) { setDraftIn(from, text); toast(friendlyError(e), "error"); }
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
    } catch (e) { toast(friendlyError(e), "error"); }
  }

  // Download a file message's bytes and save them via the native dialog.
  // Tracked per content hash so the card shows it is working: the request can
  // take as long as the server takes to answer, and a click with no feedback
  // is indistinguishable from a broken button.
  // Accepts both shapes: a message card names the file in `text`, a Drive row
  // in `name`. Reading only one of them saved the file as `undefined`.
  async function downloadFile(m) {
    if (!server || !m.hash || downloading[m.hash]) return;
    const filename = m.text || m.name || "file";
    setDownloading((d) => ({ ...d, [m.hash]: true }));
    try { await api.partyDownloadFile(server.id, m.hash, filename); }
    catch (e) { toast(friendlyError(e), "error"); }
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
    catch (e) { toast(friendlyError(e), "error"); }
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
    } catch (e) { toast(friendlyError(e), "error"); }
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
    } catch (e) { toast(friendlyError(e), "error"); }
  }

  async function setRole(member, role) {
    if (!server) return;
    try { await api.partySetRole(server.id, member.id, role); loadServers(); }
    catch (e) { toast(friendlyError(e), "error"); }
  }

  // Re-share a file into another channel or DM. The bytes are already on the
  // server, so this is a reference rather than an upload.
  async function shareFile(f, dest) {
    if (!server) return;
    try {
      await api.partyShareFile(server.id, f.hash, f.location, dest);
      await api.partyRefreshFiles(server.id);
      loadServers(); loadMsgs();
    } catch (e) { toast(friendlyError(e), "error"); }
  }

  async function setFilePerms(f, member, perms) {
    if (!server) return;
    try {
      await api.partySetFilePermissions(server.id, f.hash, f.location, member, perms);
      await api.partyRefreshFiles(server.id);
      loadServers();
    } catch (e) { toast(friendlyError(e), "error"); }
  }

  async function deleteFile(f) {
    if (!server) return;
    try {
      await api.partyDeleteFile(server.id, f.hash, f.location);
      await api.partyRefreshFiles(server.id);
      loadServers();
    } catch (e) { toast(friendlyError(e), "error"); }
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
    } catch (e) { toast(friendlyError(e), "error"); }
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
    } catch (e) { toast(friendlyError(e), "error"); }
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
                  markRead(server, c.id, c.messages);
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
                  markRead(server, `dm-${m.id}`, m.dm_messages);
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
              {/* Offered only where the server would actually agree: you cannot
                  change your own role, the owner's, or that of a peer at your
                  own level, and you can only grant roles below your own. */}
              {assignableRoles(server?.my_role, m).length > 0 && (
                <select className="party-rolesel" value={m.role}
                  title={`Role for ${m.username}`}
                  onChange={(e) => setRole(m, e.target.value)}>
                  {assignableRoles(server?.my_role, m).map((r) => (
                    <option key={r} value={r}>{r}</option>
                  ))}
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
                {/* Always on screen, not only at join time: community traffic is
                    stored readable by the operator, and a DM here looks like
                    any other DM (audit JOURNEY-04 / PRIV-01). */}
                <span className="set-badge is-warn party-operator-badge"
                  title="Messages, DMs and files in this community are stored on the server unencrypted. Whoever runs it — or gets into it — can read them.">
                  Operator can read messages
                </span>
              </div>
            </div>
          </div>
          <div className="party-head-r">
            <div className="party-views" role="group" aria-label="Community view">
              <button aria-pressed={view === "chat"}
                className={cx("party-view", view === "chat" && "is-active")}
                onClick={() => setView("chat")} title="Conversation">
                <Icon name="message" size={14} />
              </button>
              <button aria-pressed={view === "drive"}
                className={cx("party-view", view === "drive" && "is-active")}
                onClick={() => setView("drive")} title="Shared files">
                <Icon name="folder" size={14} />
              </button>
              {isAdmin && (
                <button aria-pressed={view === "audit"}
                  className={cx("party-view", view === "audit" && "is-active")}
                  onClick={() => setView("audit")} title="Activity log">
                  <Icon name="clock" size={14} />
                </button>
              )}
            </div>
            {/* A truncated fingerprint can't be checked; the button reveals and
                copies the whole pinned key. */}
            <button className="party-fp" title={`Server key pinned. Full fingerprint: ${server?.fingerprint || ""}`}
              aria-label="Copy the server's pinned fingerprint"
              onClick={() => navigator.clipboard?.writeText(server?.fingerprint || "")
                .then(() => toast("Server fingerprint copied", "success"))
                .catch(() => toast("Could not copy", "error"))}>
              <Icon name="lock" size={13} />
              <span>Key pinned</span>
              <Icon name="copy" size={12} />
            </button>
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
            onDownload={downloadFile} onDelete={deleteFile}
            onShare={setShareFor} onPermissions={setPermsFor} />
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

            {dm && (
              <div className="party-dm-note" role="note">
                <Icon name="alert" size={13} /> Community DMs are not private from the
                server: its operator can read them. For a private conversation, connect
                to this person directly.
              </div>
            )}
            <div className="composer">
              <button className="composer-clip" onClick={sendFile} title="Share a file"
                disabled={!canSend}>
                <Icon name="paperclip" size={18} />
              </button>
              <textarea className="composer-input" rows={1}
                placeholder={composerHint}
                // The placeholder is the only thing that says *why* the box is
                // disabled, and a placeholder is not an accessible name.
                aria-label={composerHint}
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

      {permsFor && (
        <FilePermissionsDialog server={server} file={permsFor}
          onClose={() => setPermsFor(null)}
          onSubmit={(member, perms) => setFilePerms(permsFor, member, perms)} />
      )}

      {shareFor && (
        <ShareFileDialog server={server} file={shareFor}
          onClose={() => setShareFor(null)}
          onSubmit={(dest) => shareFile(shareFor, dest)} />
      )}

      {accessFor && (
        <ChannelAccessDialog server={server}
          channel={accessFor === "new" ? null : accessFor}
          onClose={() => setAccessFor(null)}
          onSubmit={submitAccess} />
      )}
    </div>
  );
}
