// Thin typed wrappers over the Tauri command bridge + adapters that map the
// Rust `ChatManager` shapes (Chat/ConvSummary/Message) into the presentational
// `contact` shape the ported components expect.
//
// When NOT running inside Tauri (e.g. opened in a plain browser for UI work),
// `api`/`onBridge` fall back to an in-memory mock so the whole UI is navigable
// without the Rust backend. Append `?mock=unlock` / `?mock=set_password` to the
// URL to preview the auth screens.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const inTauri = typeof window !== "undefined" && !!window.__TAURI_INTERNALS__;

const realApi = {
  authStatus: () => invoke("auth_status"),
  unlock: (password) => invoke("unlock", { password }),
  setPassword: (password) => invoke("set_password", { password }),
  setDisplayName: (name) => invoke("set_display_name", { name }),
  exportIdentity: () => invoke("export_identity"),
  listConversations: () => invoke("list_conversations"),
  getConversation: (id) => invoke("get_conversation", { id }),
  sendMessage: (id, text) => invoke("send_message", { id, text }),
  sendFile: (id) => invoke("send_file", { id }),
  listTransfers: () => invoke("list_transfers"),
  acceptTransfer: (id) => invoke("accept_transfer", { id }),
  declineTransfer: (id) => invoke("decline_transfer", { id }),
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
  partyJoin: (address, username, password) => invoke("party_join", { address, username, password }),
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
  const summaries = () => Object.values(chats).map((c) => ({
    id: c.id, title: c.title,
    last: c.messages.length ? (c.messages.at(-1).content.text || "") : null,
    connected: !!connected[c.id], placeholder: c.is_host_placeholder,
    verified: !!c.peer_fingerprint,
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
      channels: [{ id: gen, name: "general", messages: 0 }, { id: rnd, name: "random", messages: 0 }],
      members: [
        { id: me, username: username || "you", online: true, is_me: true, dm_messages: 0 },
        { id: "mem-nova", username: "nova", online: true, is_me: false, dm_messages: 0 },
        { id: "mem-kite", username: "kite", online: false, is_me: false, dm_messages: 0 },
      ],
    }];
    partyState.msgs[`${sid}|${gen}`] = [
      { sender_name: "nova", from_me: false, kind: "text", text: "welcome to the community 👋", size: null, timestamp: Date.now() - 60000 },
    ];
    partyState.msgs[`${sid}|${rnd}`] = [];
  }
  return {
    authStatus: async () => ({ state: authState, name: "Maya", fingerprint: "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2" }),
    unlock: async () => { authState = "ready"; },
    setPassword: async () => { authState = "ready"; },
    setDisplayName: async (name) => ({ state: authState, name, fingerprint: "A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4E5F6A1B2" }),
    exportIdentity: async () => "C:\\Users\\you\\p2pem-identity-backup.json",
    listConversations: async () => summaries(),
    getConversation: async (id) => chats[id],
    sendMessage: async (id, text) => { chats[id].messages.push({ id: "x" + Math.random(), from_me: true, content: { type: "text", text }, timestamp: new Date().toISOString() }); },
    sendFile: async (id) => { chats[id].messages.push({ id: "x" + Math.random(), from_me: true, content: { type: "file", filename: "example.pdf", size: 248000 }, timestamp: new Date().toISOString() }); },
    listTransfers: async () => [],
    acceptTransfer: async () => {},
    declineTransfer: async () => {},
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
    importInvite: async () => ({ id: "c3", name: "Imported", fingerprint: "abc", address: "1.2.3.4:12345", trust: "unverified" }),
    connectContact: ok,
    partyJoin: async (address, username) => { seedParty(address, username); return partyState.servers[0].id; },
    partyList: async () => partyState.servers,
    partyHistory: async (server, channel) => partyState.msgs[`${server}|${channel}`] || [],
    partyPost: async (server, channel, text) => {
      (partyState.msgs[`${server}|${channel}`] ||= []).push({ sender_name: "you", from_me: true, kind: "text", text, size: null, timestamp: Date.now() });
    },
    partyCreateChannel: async (server, name) => {
      const s = partyState.servers.find((x) => x.id === server);
      if (s) { const id = "ch-" + Math.random().toString(36).slice(2, 7); s.channels.push({ id, name }); partyState.msgs[`${server}|${id}`] = []; }
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
  };
}

export const api = inTauri ? realApi : makeMock();
export const onBridge = inTauri ? (event, cb) => listen(event, cb) : async () => () => {};

// ── Adapters + formatting ────────────────────────────────────────────────────
export function fmtTime(ts) {
  try { return new Date(ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }); }
  catch { return ""; }
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
    lastT: "", typing: false,
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
        return { kind: "file", from: m.from_me ? "me" : "them", name: c.filename, size: human(c.size), progress: 100, t: fmtTime(m.timestamp), delivered: !!m.delivered };
      }
      return { from: m.from_me ? "me" : "them", text: msgText(c), t: fmtTime(m.timestamp), delivered: !!m.delivered };
    }),
  };
}
