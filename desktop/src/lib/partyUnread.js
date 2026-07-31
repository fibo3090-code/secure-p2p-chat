// Shared unread bookkeeping for Communities, used by both App (rail badge
// total) and the Parties pane (per-channel / per-DM badges, mark-read). One
// module-level store so the two components never disagree.
//
// Keys: `${serverId}|${channelId}` for channels, `${serverId}|dm-${memberId}`
// for DM threads.
//
// The read marks are **persisted**. A community server holds durable history and
// replays it on rejoin, so an in-memory store meant every restart re-seeded the
// baseline to the current count — silently marking everything that arrived while
// you were away as read. That is the same core-loop failure the direct-message
// unread counts had, and it is fixed the same way: remember what was read, not
// what happened to be on screen this session.
//
// Storage is `localStorage`, holding server ids, thread ids, and message counts.
// That is not a new exposure: the desktop app already keeps the joined servers'
// addresses, usernames, and pinned fingerprints in a plaintext `parties.json`
// beside the encrypted history. No message content is stored here. Direct-message
// read marks, which *are* tied to private conversations, live in the encrypted
// history instead (`Chat::read_count`).

const STORAGE_KEY = "p2pem.party.read";

/** Load the persisted marks, tolerating absent/corrupt/partial storage. */
function load() {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY);
    if (!raw) return new Map();
    const obj = JSON.parse(raw);
    if (!obj || typeof obj !== "object") return new Map();
    return new Map(
      Object.entries(obj).filter(([, v]) => Number.isInteger(v) && v >= 0),
    );
  } catch {
    // Unreadable storage must never break the Communities pane — losing the
    // marks only means a round of over-reporting, which is the safe direction.
    return new Map();
  }
}

const seen = load();

function persist() {
  try {
    globalThis.localStorage?.setItem(
      STORAGE_KEY,
      JSON.stringify(Object.fromEntries(seen)),
    );
  } catch { /* storage full or unavailable — badges still work this session */ }
}

function threadKeys(server) {
  const keys = [];
  for (const c of server.channels || []) {
    keys.push({ key: `${server.id}|${c.id}`, count: c.messages ?? 0 });
  }
  for (const m of server.members || []) {
    if (!m.is_me) keys.push({ key: `${server.id}|dm-${m.id}`, count: m.dm_messages ?? 0 });
  }
  return keys;
}

// Mark one thread fully read at `count` messages.
export function markRead(serverId, threadId, count) {
  const key = `${serverId}|${threadId}`;
  const n = Number.isInteger(count) && count >= 0 ? count : 0;
  if (seen.get(key) === n) return;
  seen.set(key, n);
  persist();
}

// Compute unread for a server list, returning { total, byKey }.
//
// A thread with no stored mark counts as **fully unread** rather than being
// seeded to its current count. Seeding was what hid messages that arrived while
// the app was closed; over-reporting on a genuinely new thread is the correct
// direction to err, and one click clears it.
export function computeUnread(servers) {
  let total = 0;
  const byKey = {};
  for (const s of servers || []) {
    for (const { key, count } of threadKeys(s)) {
      const n = Math.max(0, count - (seen.get(key) ?? 0));
      if (n > 0) byKey[key] = n;
      total += n;
    }
  }
  return { total, byKey };
}

// Drop marks for servers the user has left, so storage does not grow without
// bound as communities come and go.
export function pruneTo(servers) {
  const live = new Set((servers || []).map((s) => String(s.id)));
  let changed = false;
  for (const key of [...seen.keys()]) {
    if (!live.has(key.split("|")[0])) {
      seen.delete(key);
      changed = true;
    }
  }
  if (changed) persist();
}

// Test seam: drop every mark (the module otherwise keeps one process-wide store).
export function _resetForTests() {
  seen.clear();
  persist();
}
