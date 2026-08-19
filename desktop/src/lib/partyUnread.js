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

import { read, write } from "./localStore.js";

const STORAGE_KEY = "p2pem.party.read";

/** A stored blob is only a read-mark map if every entry is a count. */
function isMarkMap(obj) {
  if (!obj || typeof obj !== "object" || Array.isArray(obj)) return false;
  return Object.values(obj).every((v) => Number.isInteger(v) && v >= 0);
}

/// Load the persisted marks, tolerating absent/corrupt/partial/tampered storage.
///
/// Goes through `localStore`, so a damaged entry — or a blob lifted from another
/// key — is rejected outright rather than parsed for whatever survives. Entries
/// that are not counts are dropped individually, since one bad key must not cost
/// the user every other thread's mark.
///
/// Every failure lands in the same place: an empty map, which over-reports
/// unread. That is the safe direction — it shows messages that were already
/// read, rather than hiding ones that were not.
function load() {
  const stored = read(
    STORAGE_KEY,
    isMarkMap,
    // `legacy`: marks used to be a bare JSON object with no envelope.
    (raw) => {
      const obj = JSON.parse(raw);
      if (!obj || typeof obj !== "object" || Array.isArray(obj)) return undefined;
      return Object.fromEntries(
        Object.entries(obj).filter(([, v]) => Number.isInteger(v) && v >= 0),
      );
    },
  );
  return new Map(Object.entries(stored ?? {}));
}

const seen = load();

function persist() {
  write(STORAGE_KEY, Object.fromEntries(seen));
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
