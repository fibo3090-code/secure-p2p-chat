// Shared unread bookkeeping for Communities, used by both App (rail badge
// total) and the Parties pane (per-channel / per-DM badges, mark-read). One
// module-level store so the two components never disagree.
//
// Keys: `${serverId}|${channelId}` for channels, `${serverId}|dm-${memberId}`
// for DM threads. A key's first sighting seeds its baseline to the current
// count, so pre-existing history is never shown as unread.

const seen = new Map();

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
  seen.set(`${serverId}|${threadId}`, count ?? 0);
}

// Compute unread for a server list: seeds unseen keys, returns
// { total, byKey } where byKey maps thread keys to their unread count.
export function computeUnread(servers) {
  let total = 0;
  const byKey = {};
  for (const s of servers || []) {
    for (const { key, count } of threadKeys(s)) {
      if (!seen.has(key)) {
        seen.set(key, count); // first sighting: old history is not unread
        continue;
      }
      const n = Math.max(0, count - seen.get(key));
      if (n > 0) byKey[key] = n;
      total += n;
    }
  }
  return { total, byKey };
}
