// Thin typed wrappers over the Tauri command bridge + adapters that map the
// Rust `ChatManager` shapes (Chat/ConvSummary/Message) into the presentational
// `contact` shape the ported components expect.
//
// When NOT running inside Tauri (e.g. opened in a plain browser for UI work),
// `api`/`onBridge` fall back to an in-memory mock so the whole UI is navigable
// without the Rust backend. Append `?mock=unlock`, `?mock=set_password`, or
// `?mock=error` to the URL to preview the auth and startup-failure screens.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { folderOf } from "./parse.js";

const inTauri = typeof window !== "undefined" && !!window.__TAURI_INTERNALS__;

const realApi = {
  authStatus: () => invoke("auth_status"),
  unlock: (password) => invoke("unlock", { password }),
  setPassword: (password) => invoke("set_password", { password }),
  changePassword: (current, next) => invoke("change_password", { current, new: next }),
  setDisplayName: (name) => invoke("set_display_name", { name }),
  exportIdentity: () => invoke("export_identity"),
  exportDiagnostics: () => invoke("export_diagnostics"),
  openDataDir: () => invoke("open_data_dir"),
  lockState: () => invoke("lock_state"),
  setLocked: (locked) => invoke("set_locked", { locked }),
  listConversations: () => invoke("list_conversations"),
  getConversation: (id) => invoke("get_conversation", { id }),
  markRead: (id) => invoke("mark_read", { id }),
  // Window focus + open conversation, so the backend can tell a background
  // message from one the user is reading. Single-word params by convention.
  setPresence: (focused, chat) => invoke("set_presence", { focused, chat: chat || null }),
  sendMessage: (id, text) => invoke("send_message", { id, text }),
  sendFile: (id) => invoke("send_file", { id }),
  // File cards: open with the default app (reveal=false) or show in folder.
  // Only (chat id, message id) cross the bridge — never filesystem paths.
  // Resolves to { opened, blocked, filename }. `blocked` is set when the file
  // came from the peer and opening it would execute code — pass confirm:true
  // to go ahead once the user has been told what it is.
  openFile: (id, msg, reveal = false, confirm = false) =>
    invoke("open_file", { id, msg, reveal, confirm }),
  filePreview: (id, msg) => invoke("file_preview", { id, msg }),
  openUrl: (url) => invoke("open_url", { url }),
  listTransfers: () => invoke("list_transfers"),
  acceptTransfer: (id) => invoke("accept_transfer", { id }),
  declineTransfer: (id) => invoke("decline_transfer", { id }),
  cancelTransfer: (id) => invoke("cancel_transfer", { id }),
  getSettings: () => invoke("get_settings"),
  updateSettings: (settings) => invoke("update_settings", { settings }),
  pickDownloadDir: () => invoke("pick_download_dir"),
  startHost: (port, password) => invoke("start_host", { port, password: password || null }),
  connectPeer: (host, port, password) => invoke("connect_peer", { host, port, password: password || null }),
  listDiscoveredPeers: () => invoke("list_discovered_peers"),
  myAddresses: () => invoke("my_addresses"),
  hostViaRelay: (relay) => invoke("host_via_relay", { relay }),
  connectViaRelay: (relay, token) => invoke("connect_via_relay", { relay, token }),
  confirmFingerprint: (chatId, accept) => invoke("confirm_fingerprint", { id: chatId, accept }),
  pendingFingerprint: () => invoke("pending_fingerprint"),
  renameChat: (id, title) => invoke("rename_chat", { id, title }),
  deleteChat: (id) => invoke("delete_chat", { id }),
  listContacts: () => invoke("list_contacts"),
  removeContact: (id) => invoke("remove_contact", { id }),
  blockContact: (id) => invoke("block_contact", { id }),
  unblockContact: (id) => invoke("unblock_contact", { id }),
  myInviteLink: () => invoke("my_invite_link"),
  importInvite: (link) => invoke("import_invite", { link }),
  connectContact: (id) => invoke("connect_contact", { id }),
  // Communities (Party servers). Single-word command params by convention.
  // `trust` is the second step of first-join verification: false asks the
  // bridge to stop and report the server fingerprint + SAS without sending
  // the credentials; true proceeds after the user has compared them.
  partyJoin: (address, username, password, trust = false) =>
    invoke("party_join", { address, username, password, trust }),
  partyList: () => invoke("party_list"),
  partyHistory: (server, channel) => invoke("party_history", { server, channel }),
  partyPost: (server, channel, text) => invoke("party_post", { server, channel, text }),
  partyCreateChannel: (server, name) => invoke("party_create_channel", { server, name }),
  partySendDm: (server, to, text) => invoke("party_send_dm", { server, to, text }),
  partyDmHistory: (server, peer) => invoke("party_dm_history", { server, peer }),
  partyClearError: (server) => invoke("party_clear_error", { server }),
  partySendFile: (server, channel) => invoke("party_send_file", { server, channel }),
  partySendFileDm: (server, to) => invoke("party_send_file_dm", { server, to }),
  partyDownloadFile: (server, hash, name) => invoke("party_download_file", { server, hash, name }),
  partySaved: () => invoke("party_saved"),
  partyLeave: (server) => invoke("party_leave", { server }),
  partyCreateChannelKind: (server, name, kind, members = []) =>
    invoke("party_create_channel_kind", { server, name, kind, members }),
  partyDeleteChannel: (server, channel) => invoke("party_delete_channel", { server, channel }),
  partySetChannelAccess: (server, channel, kind, members = []) =>
    invoke("party_set_channel_access", { server, channel, kind, members }),
  partySetRole: (server, member, role) => invoke("party_set_role", { server, member, role }),
  partyRefreshFiles: (server) => invoke("party_refresh_files", { server }),
  partyDeleteFile: (server, hash, location) =>
    invoke("party_delete_file", { server, hash, location }),
  partyRefreshAudit: (server) => invoke("party_refresh_audit", { server }),
  partyClearNotice: (server) => invoke("party_clear_notice", { server }),
  partyShareFile: (server, hash, from, { channel = null, peer = null } = {}) =>
    invoke("party_share_file", { server, hash, from, channel, peer }),
  partySetFilePermissions: (server, hash, location, member, perms) =>
    invoke("party_set_file_permissions", { server, hash, location, member, perms }),
};

// ── Dev mock ────────────────────────────────────────────────────────────────
function makeMock() {
  const now = new Date().toISOString();
  let authState = new URLSearchParams(location.search).get("mock") || "ready";
  const mockSettings = {
    download_dir: "~/Downloads", enable_notifications: true,
    enable_typing_indicators: true, auto_host_on_startup: false, listen_port: 12345, enable_upnp: false,
    auto_accept_files: false, auto_connect: false, enable_mdns: false,
  };
  const chats = {
    "11111111-1111-1111-1111-111111111111": {
      id: "11111111-1111-1111-1111-111111111111", title: "Alice", peer_fingerprint: "a1b2c3d4e5f60718293a4b5c6d7e8f90112233445566778899aabbccddeeff00",
      participants: [], created_at: now, is_host_placeholder: false,
      messages: [
        { id: "m1", from_me: false, content: { type: "text", text: "hey! is this thing encrypted end to end?" }, timestamp: now },
        { id: "m2", from_me: true, content: { type: "text", text: "yep — X25519 + AES-GCM. fingerprint verified ✔" }, timestamp: now },
        { id: "m3", from_me: false, content: { type: "text", text: "slick. sending you the file now" }, timestamp: now },
      ],
    },
    "22222222-2222-2222-2222-222222222222": {
      id: "22222222-2222-2222-2222-222222222222", title: "Bob", peer_fingerprint: "ffeeddccbbaa00998877665544332211f0e1d2c3b4a5968778695a4b3c2d1e0f",
      participants: [], created_at: now, is_host_placeholder: false, messages: [
        { id: "m4", from_me: false, content: { type: "text", text: "let's verify fingerprints" }, timestamp: now },
      ],
    },
    "33333333-3333-3333-3333-333333333333": {
      id: "33333333-3333-3333-3333-333333333333", title: "Listening :12345", peer_fingerprint: null,
      participants: [], created_at: now, is_host_placeholder: true, messages: [],
    },
  };
  const connected = { "11111111-1111-1111-1111-111111111111": true };
  const contacts = [
    { id: "c1", name: "Alice", fingerprint: "a1b2c3d4e5f6", address: "192.168.1.21:12345", trust: "verified" },
    { id: "c2", name: "Carol", fingerprint: "deadbeef0102", address: "10.0.0.5:12345", trust: "unverified" },
  ];
  // Mirrors the real bridge: unread is derived from a persisted read mark, not
  // from what the UI happened to see this session.
  const readMarks = {};
  const summaries = () => Object.values(chats).map((c) => ({
    id: c.id, title: c.title,
    last: c.messages.length ? (c.messages.at(-1).content.text || "") : null,
    connected: !!connected[c.id], placeholder: c.is_host_placeholder,
    verified: !!c.peer_fingerprint,
    messages: c.messages.length,
    unread: c.messages.slice(readMarks[c.id] ?? 0).filter((m) => !m.from_me).length,
    last_at: c.messages.length ? c.messages.at(-1).timestamp : null,
  }));
  const ok = async () => {};
  // ── Party (Communities) mock — a single joinable server with two channels ──
  const partyState = { servers: [], msgs: {} }; // msgs keyed by `${server}|${thread}`
  function seedParty(address, username) {
    const sid = "srv-" + Math.random().toString(36).slice(2, 8);
    const me = "mem-me";
    const gen = "ch-general", rnd = "ch-random";
    partyState.servers = [{
      id: sid, name: address.split(":")[0] || "Community", address,
      username: username || "you",
      fingerprint: "5f3a9c2e7b1d4068aa22cc55ee88ff00112233445566778899aabbccddeeff11",
      status: "joined", status_detail: null, member_id: me, last_error: null,
      // The mock signs you in as the owner so the admin surfaces are reachable
      // in a plain browser; the real server decides this, not the client.
      my_role: "owner", last_notice: null,
      channels: [
        { id: gen, name: "general", messages: 0, kind: "public", members: [], can_post: true },
        { id: rnd, name: "random", messages: 0, kind: "public", members: [], can_post: true },
      ],
      members: [
        { id: me, username: username || "you", online: true, is_me: true, dm_messages: 0, role: "owner" },
        { id: "mem-nova", username: "nova", online: true, is_me: false, dm_messages: 0, role: "member" },
        { id: "mem-kite", username: "kite", online: false, is_me: false, dm_messages: 0, role: "guest" },
      ],
      files: [
        {
          hash: "mockhash", name: "roadmap.pdf", size: 284134, mime: "application/pdf",
          uploader: "mem-nova", uploader_name: "nova", location: gen, location_name: "#general",
          is_dm: false, shared_at: Date.now() - 3600000, can_delete: true,
          can_view: true, can_download: true, can_share: true,
        },
      ],
      quota: { used: 284134, limit: 134217728, server_used: 284134, server_limit: 1073741824 },
      audit: [
        { at: Date.now() - 7200000, actor_name: username || "you", action: "channel.create", detail: "created public channel #random" },
      ],
    }];
    partyState.msgs[`${sid}|${gen}`] = [
      { sender_name: "nova", from_me: false, kind: "text", text: "welcome to the community 👋", size: null, timestamp: Date.now() - 60000 },
    ];
    partyState.msgs[`${sid}|${rnd}`] = [];
  }
  return {
    // `?mock=error` previews the unrecoverable-startup screen.
    authStatus: async () => ({
      state: authState, name: "Maya", min_password_len: 12,
      error: authState === "error" ? "Your identity file at C:\\…\\identity.json exists but could not be read." : null,
      fingerprint: "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2",
    }),
    unlock: async () => { authState = "ready"; },
    setPassword: async () => { authState = "ready"; },
    changePassword: async (current) => {
      // The mock has no key material; reject an obviously-empty current password
      // so the dialog's error path is exercisable in a plain browser.
      if (!current) throw new Error("Current password is incorrect");
    },
    setDisplayName: async (name) => ({ state: authState, name, fingerprint: "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2" }),
    exportIdentity: async () => "C:\\Users\\you\\p2pem-identity-backup.json",
    exportDiagnostics: async () => "C:\\Users\\you\\AppData\\P2PEM\\diagnostics\\bundle-mock",
    openDataDir: async () => "C:\\Users\\you\\AppData\\P2PEM",
    lockState: async () => mockSettings._locked || false,
    setLocked: async (locked) => { mockSettings._locked = locked; },
    listConversations: async () => summaries(),
    getConversation: async (id) => chats[id],
    markRead: async (id) => { if (chats[id]) readMarks[id] = chats[id].messages.length; },
    setPresence: async () => {},
    sendMessage: async (id, text) => { chats[id].messages.push({ id: "x" + Math.random(), from_me: true, content: { type: "text", text }, timestamp: new Date().toISOString() }); },
    sendFile: async (id) => { chats[id].messages.push({ id: "x" + Math.random(), from_me: true, content: { type: "file", filename: "example.pdf", size: 248000, path: "/mock/example.pdf" }, timestamp: new Date().toISOString() }); },
    openFile: async () => ({ opened: true, blocked: null, filename: null }),
    filePreview: async () => null,
    openUrl: async (url) => { window.open(url, "_blank", "noopener"); },
    listTransfers: async () => [],
    acceptTransfer: async (_id) => {},
    declineTransfer: async (_id) => {},
    cancelTransfer: async (_id) => {},
    getSettings: async () => mockSettings,
    updateSettings: async (s) => { Object.assign(mockSettings, s); },
    pickDownloadDir: async () => { mockSettings.download_dir = "C:\\Users\\you\\Downloads"; return mockSettings.download_dir; },
    startHost: ok, connectPeer: ok, confirmFingerprint: ok,
    listDiscoveredPeers: async () => (mockSettings.enable_mdns
      ? { enabled: true, peers: [{ name: "laptop-alice", address: "192.168.1.21", port: 12345, fingerprint: "a1b2c3" }] }
      : { enabled: false, peers: [] }),
    myAddresses: async () => ({ hosting: true, local: "192.168.1.9:12345", external: null }),
    hostViaRelay: async () => "rly_" + Math.random().toString(36).slice(2, 10),
    connectViaRelay: ok,
    pendingFingerprint: async () => null,
    renameChat: async (id, title) => { if (chats[id]) chats[id].title = title; },
    deleteChat: async (id) => { delete chats[id]; },
    listContacts: async () => contacts,
    removeContact: async (id) => { const i = contacts.findIndex((c) => c.id === id); if (i > -1) contacts.splice(i, 1); },
    blockContact: async (id) => { const c = contacts.find((x) => x.id === id); if (c) c.trust = "blocked"; },
    unblockContact: async (id) => { const c = contacts.find((x) => x.id === id); if (c) c.trust = "unverified"; },
    myInviteLink: async () => "chat-p2p://invite/eyJuYW1lIjoiTWF5YSIsImFkZHJlc3MiOiIxOTIuMTY4LjEuOToxMjM0NSJ9",
    importInvite: async () => ({
      contact: { id: "c3", name: "Imported", fingerprint: "abc", address: "1.2.3.4:12345", trust: "unverified", reachable: true, relay_only: false, blocked: false },
      signed: true,
    }),
    connectContact: ok,
    partyJoin: async (address, username, password, trust = false) => {
      if (!trust) return { status: "verify", fingerprint: "aa".repeat(32), sas: "123456 🎃🎈🎁" };
      seedParty(address, username);
      return { status: "joined", server: partyState.servers[0].id, fingerprint: "aa".repeat(32) };
    },
    partyList: async () => partyState.servers,
    partyHistory: async (server, channel) => partyState.msgs[`${server}|${channel}`] || [],
    partyPost: async (server, channel, text) => {
      (partyState.msgs[`${server}|${channel}`] ||= []).push({ sender_name: "you", from_me: true, kind: "text", text, size: null, timestamp: Date.now() });
    },
    partyCreateChannel: async (server, name) => {
      const s = partyState.servers.find((x) => x.id === server);
      if (s) { const id = "ch-" + Math.random().toString(36).slice(2, 7); s.channels.push({ id, name, messages: 0, kind: "public", members: [], can_post: true }); partyState.msgs[`${server}|${id}`] = []; }
    },
    partySendDm: async (server, to, text) => {
      const key = `${server}|dm-${to}`;
      (partyState.msgs[key] ||= []).push({ sender_name: "you", from_me: true, kind: "text", text, size: null, timestamp: Date.now() });
    },
    partyDmHistory: async (server, peer) => partyState.msgs[`${server}|dm-${peer}`] || [],
    partyClearError: async (server) => { const s = partyState.servers.find((x) => x.id === server); if (s) s.last_error = null; },
    partySendFile: async (server, channel) => {
      (partyState.msgs[`${server}|${channel}`] ||= []).push({ sender_name: "you", from_me: true, kind: "file", text: "mock-file.png", size: 12345, hash: "mockhash", timestamp: Date.now() });
    },
    partySendFileDm: async (server, to) => {
      (partyState.msgs[`${server}|dm-${to}`] ||= []).push({ sender_name: "you", from_me: true, kind: "file", text: "mock-file.png", size: 12345, hash: "mockhash", timestamp: Date.now() });
    },
    partyDownloadFile: async () => { console.log("[mock] party download only works in the desktop app"); },
    partySaved: async () => [{ address: "192.168.1.20:12345", username: "you", name: "Mock Community", fingerprint: "abc123" }],
    partyLeave: async (server) => { partyState.servers = partyState.servers.filter((s) => s.id !== server); },
    partyCreateChannelKind: async (server, name, kind, members = []) => {
      const s = partyState.servers.find((x) => x.id === server);
      if (!s) return;
      const id = "ch-" + Math.random().toString(36).slice(2, 7);
      s.channels.push({ id, name, messages: 0, kind, members, can_post: true });
      partyState.msgs[`${server}|${id}`] = [];
    },
    partyDeleteChannel: async (server, channel) => {
      const s = partyState.servers.find((x) => x.id === server);
      if (s) s.channels = s.channels.filter((c) => c.id !== channel);
      delete partyState.msgs[`${server}|${channel}`];
    },
    partySetChannelAccess: async (server, channel, kind, members = []) => {
      const c = partyState.servers.find((x) => x.id === server)?.channels.find((c) => c.id === channel);
      if (c) { c.kind = kind; c.members = members; }
    },
    partySetRole: async (server, member, role) => {
      const m = partyState.servers.find((x) => x.id === server)?.members.find((m) => m.id === member);
      if (m) m.role = role;
    },
    partyRefreshFiles: ok,
    partyDeleteFile: async (server, hash, location) => {
      const s = partyState.servers.find((x) => x.id === server);
      if (s) s.files = s.files.filter((f) => !(f.hash === hash && f.location === location));
    },
    partyRefreshAudit: ok,
    partyClearNotice: async (server) => {
      const s = partyState.servers.find((x) => x.id === server);
      if (s) s.last_notice = null;
    },
    partyShareFile: async (server, hash, _from, { channel = null } = {}) => {
      const s = partyState.servers.find((x) => x.id === server);
      const src = s?.files.find((f) => f.hash === hash);
      if (!s || !src) return;
      const target = s.channels.find((c) => c.id === channel);
      s.files.push({ ...src, location: channel, location_name: `#${target?.name || "?"}` });
    },
    partySetFilePermissions: async (server, hash, location, member, perms) => {
      const f = partyState.servers
        .find((x) => x.id === server)?.files
        .find((f) => f.hash === hash && f.location === location);
      if (!f || member) return; // the mock only models the default grant
      f.can_view = perms.view;
      f.can_download = perms.download;
      f.can_delete = perms.delete;
      f.can_share = perms.share;
    },
  };
}

export const api = inTauri ? realApi : makeMock();
export const onBridge = inTauri ? (event, cb) => listen(event, cb) : async () => () => {};

// ── Adapters + formatting ────────────────────────────────────────────────────
export function fmtTime(ts) {
  try { return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }); }
  catch { return ""; }
}

// Compact "last activity" stamp for the conversation list: time-of-day for
// today, weekday within the last week, short date otherwise.
export function fmtWhen(ts) {
  if (!ts) return "";
  try {
    const d = new Date(ts);
    if (isNaN(d)) return "";
    const now = new Date();
    if (d.toDateString() === now.toDateString()) return fmtTime(d);
    const days = (now - d) / 86400000;
    if (days < 7) return d.toLocaleDateString([], { weekday: "short" });
    return d.toLocaleDateString([], { month: "short", day: "numeric" });
  } catch { return ""; }
}

// Label for day separators inside a thread.
export function fmtDay(ts) {
  const d = new Date(ts);
  if (isNaN(d)) return "";
  const today = new Date();
  const yesterday = new Date(today.getTime() - 86400000);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });
}

function msgText(c) {
  if (!c) return "";
  if (c.type === "text") return c.text;
  if (c.type === "file") return c.filename;
  return c.text || "";
}

function human(n) {
  if (n == null) return "";
  const u = ["B", "KB", "MB", "GB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return n.toFixed(i ? 1 : 0) + " " + u[i];
}

// Both adapters normalize kind/transport identically (backends emit either
// casing across summary vs detail payloads) so a conversation keeps the same
// shape whichever payload arrived last.
function normalizeKind(value) { return String(value || "dm").toLowerCase(); }
function normalizeTransport(value) { return String(value || "direct").toLowerCase(); }

export function summaryToConv(s) {
  const transport = normalizeTransport(s.transport);
  return {
    id: s.id, name: s.title,
    last: s.last || (s.connected ? "Connected" : s.placeholder ? "Waiting for a peer…" : ""),
    lastT: fmtWhen(s.last_at), typing: false,
    kind: normalizeKind(s.kind),
    transport,
    relay: transport === "relay" || transport === "server",
    state: s.connected ? "connected" : s.placeholder ? "hosting" : "offline",
    // Only claim "verified" once the fingerprint was actually confirmed.
    trust: s.verified ? "verified" : "unverified", unread: s.unread ?? 0, placeholder: s.placeholder,
  };
}

export function chatToContact(chat, connected) {
  if (!chat) return null;
  const transport = normalizeTransport(chat.transport);
  return {
    id: chat.id, name: chat.title,
    state: connected ? "connected" : chat.is_host_placeholder ? "hosting" : "offline",
    // A stored peer fingerprint means TOFU verification has completed.
    trust: chat.peer_fingerprint ? "verified" : "unverified",
    fingerprint: chat.peer_fingerprint || "",
    address: chat.peer_fingerprint ? chat.peer_fingerprint.slice(0, 16) + "…" : "",
    placeholder: chat.is_host_placeholder,
    kind: normalizeKind(chat.kind),
    transport,
    relay: transport === "relay" || transport === "server",
    members: 0, typing: false,
    messages: (chat.messages || []).map((m) => {
      const c = m.content || {};
      if (c.type === "file") {
        // hasPath gates the open/reveal actions — a file whose location was
        // never recorded (old history) renders as a plain card.
        return { kind: "file", id: m.id, ts: m.timestamp, from: m.from_me ? "me" : "them", name: c.filename, size: human(c.size), progress: 100, t: fmtTime(m.timestamp), hasPath: !!c.path, path: c.path ? String(c.path) : "", dir: folderOf(c.path), delivered: !!m.delivered };
      }
      return { id: m.id, ts: m.timestamp, from: m.from_me ? "me" : "them", text: msgText(c), t: fmtTime(m.timestamp), delivered: !!m.delivered };
    }),
  };
}
