// Shared unread bookkeeping for Communities (App's rail badge and the Parties
// pane), in one module-level store so the two never disagree. Keys:
// `${serverId}|${channelId}` and `${serverId}|dm-${memberId}`.
//
// Keys: `${anchor}|${channelId}` for channels, `${anchor}|dm-${memberId}` for
// DM threads, where the anchor is the server's pinned **fingerprint**: the `id`
// a server has in the list is minted fresh on every join, so marks keyed by it
// never survived a restart (UI-05). `computeUnread`'s `byKey` is keyed by the
// live `id`, which is what the panes look threads up by.
//
// Read marks are **persisted** (remember what was read, not what was on
// screen), or every restart would mark everything that arrived while away as
// read. `localStorage` holds only ids and counts, never content; DM read marks
// live in the encrypted history (`Chat::read_count`).

import { read, write } from "./localStore.js";

const STORAGE_KEY = "p2pem.party.read";

const isCount = (v) => Number.isInteger(v) && v >= 0;
const isPlainObject = (obj) => !!obj && typeof obj === "object" && !Array.isArray(obj);

/** A stored blob is only a read-mark map if every entry is a count. */
function isMarkMap(obj) {
  return isPlainObject(obj) && Object.values(obj).every(isCount);
}

/// Load the persisted marks. A damaged or foreign blob is rejected outright;
/// a legacy bare object keeps its valid entries. Every failure is an empty map,
/// which over-reports unread — the safe direction.
function load() {
  const stored = read(STORAGE_KEY, isMarkMap, (raw) => {
    const obj = JSON.parse(raw);
    if (!isPlainObject(obj)) return undefined;
    return Object.fromEntries(Object.entries(obj).filter(([, v]) => isCount(v)));
  });
  return new Map(Object.entries(stored ?? {}));
}

const seen = load();

function persist() {
  write(STORAGE_KEY, Object.fromEntries(seen));
}

/** What a server's marks are stored under: its fingerprint, or its id if it has none. */
function anchorOf(server) {
  return String(server?.fingerprint || server?.id);
}

function threadKeys(server) {
  const keys = [];
  const anchor = anchorOf(server);
  for (const c of server.channels || []) {
    keys.push({ shown: `${server.id}|${c.id}`, stored: `${anchor}|${c.id}`, count: c.messages ?? 0 });
  }
  for (const m of server.members || []) {
    if (!m.is_me) {
      keys.push({
        shown: `${server.id}|dm-${m.id}`,
        stored: `${anchor}|dm-${m.id}`,
        count: m.dm_messages ?? 0,
      });
    }
  }
  return keys;
}

// Mark one thread of `server` fully read at `count` messages.
export function markRead(server, threadId, count) {
  const key = `${anchorOf(server)}|${threadId}`;
  const n = isCount(count) ? count : 0;
  if (seen.get(key) === n) return;
  seen.set(key, n);
  persist();
}

// Compute unread for a server list, returning { total, byKey }. A thread with
// no mark is **fully unread**, never seeded to its current count.
export function computeUnread(servers) {
  let total = 0;
  const byKey = {};
  for (const s of servers || []) {
    for (const { shown, stored, count } of threadKeys(s)) {
      const n = Math.max(0, count - (seen.get(stored) ?? 0));
      if (n > 0) byKey[shown] = n;
      total += n;
    }
  }
  return { total, byKey };
}

// Drop marks for communities the user has left, so storage stays bounded.
// `keep` must be the *saved* list, not only the ones connected now: pruning
// against the live list wiped the marks of every community not yet rejoined
// after a restart.
export function pruneTo(keep) {
  const live = new Set((keep || []).map(anchorOf));
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
