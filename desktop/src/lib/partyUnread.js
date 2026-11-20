// Shared unread bookkeeping for Communities, used by both App (rail badge
// total) and the Parties pane (per-channel / per-DM badges, mark-read). One
// module-level store so the two components never disagree.
//
// Stored keys: `${anchor}|${channelId}` for channels, `${anchor}|dm-${memberId}`
// for DM threads, where the anchor is the server's pinned **fingerprint**. The
// `id` a server has in the list is minted fresh on every join, so marks keyed by
// it never survived a restart or rejoin: the whole history came back badged, and
// the old marks were pruned as belonging to a server that no longer existed
// (UI-05). Channel and member ids are the server's own and are durable. The
// `byKey` map `computeUnread` returns is keyed by the live `id`, which is what
// the panes look threads up by.
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
    for (const { shown, stored, count } of threadKeys(s)) {
      const n = Math.max(0, count - (seen.get(stored) ?? 0));
      if (n > 0) byKey[shown] = n;
      total += n;
    }
  }
  return { total, byKey };
}

// Drop marks for communities the user has left, so storage does not grow
// without bound as they come and go. `keep` must name every community the user
// still has — the *saved* list, not only the ones connected right now: pruning
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
