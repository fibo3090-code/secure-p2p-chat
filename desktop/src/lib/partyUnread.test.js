import { describe, it, expect, beforeEach, vi } from "vitest";

// The module reads localStorage at import time, so the stub has to exist first.
const store = new Map();
vi.stubGlobal("localStorage", {
  getItem: (k) => (store.has(k) ? store.get(k) : null),
  setItem: (k, v) => store.set(k, String(v)),
  removeItem: (k) => store.delete(k),
});

const { computeUnread, markRead, pruneTo, _resetForTests } = await import("./partyUnread.js");

let nextId = 0;
const sid = () => `srv-${nextId++}`;

const server = (id, channelCount, dmCount = undefined) => ({
  id,
  channels: [{ id: "general", messages: channelCount }],
  members:
    dmCount === undefined
      ? []
      : [
          { id: "me", is_me: true, dm_messages: 999 },
          { id: "peer", is_me: false, dm_messages: dmCount },
        ],
});

beforeEach(() => _resetForTests());

describe("computeUnread", () => {
  it("treats an unseen thread as fully unread", () => {
    // Community servers replay history on rejoin. Seeding the baseline to the
    // current count on first sighting — the old behaviour — silently marked
    // everything that arrived while the app was closed as already read.
    const id = sid();
    const { total, byKey } = computeUnread([server(id, 40)]);
    expect(total).toBe(40);
    expect(byKey[`${id}|general`]).toBe(40);
  });

  it("counts only growth beyond the read mark", () => {
    const id = sid();
    markRead(id, "general", 10);
    const { total, byKey } = computeUnread([server(id, 13)]);
    expect(total).toBe(3);
    expect(byKey[`${id}|general`]).toBe(3);
  });

  it("never reports negative unread when counts shrink", () => {
    const id = sid();
    markRead(id, "general", 10);
    expect(computeUnread([server(id, 4)]).total).toBe(0);
  });

  it("tracks DM threads but skips the local member", () => {
    const id = sid();
    markRead(id, "dm-peer", 5);
    const { total, byKey } = computeUnread([server(id, 0, 7)]);
    expect(total).toBe(2);
    expect(byKey[`${id}|dm-peer`]).toBe(2);
    expect(Object.keys(byKey)).not.toContain(`${id}|dm-me`);
  });

  it("markRead clears a thread's unread count", () => {
    const id = sid();
    expect(computeUnread([server(id, 15)]).total).toBe(15);
    markRead(id, "general", 15);
    expect(computeUnread([server(id, 15)]).total).toBe(0);
  });
});

describe("persistence", () => {
  it("survives a reload of the module's backing store", async () => {
    const id = sid();
    markRead(id, "general", 12);
    // A fresh import reads the same localStorage, standing in for a restart.
    vi.resetModules();
    const reloaded = await import("./partyUnread.js");
    expect(reloaded.computeUnread([server(id, 12)]).total).toBe(0);
    expect(reloaded.computeUnread([server(id, 15)]).total).toBe(3);
  });

  it("survives corrupt storage without breaking the pane", async () => {
    store.set("p2pem.party.read", "{not json");
    vi.resetModules();
    const reloaded = await import("./partyUnread.js");
    // Falls back to "everything unread", which is the safe direction.
    expect(() => reloaded.computeUnread([server(sid(), 3)])).not.toThrow();
  });
});

describe("stored marks", () => {
  it("keeps marks written before the entry was checksummed", async () => {
    store.set("p2pem.party.read", JSON.stringify({ "srv-legacy|general": 3 }));
    vi.resetModules();
    const reloaded = await import("./partyUnread.js");
    const s = { id: "srv-legacy", channels: [{ id: "general", messages: 5 }], members: [] };
    expect(reloaded.computeUnread([s]).total).toBe(2);
  });

  it("discards a mark blob that has been tampered with", async () => {
    store.set("p2pem.party.read", JSON.stringify({
      v: 1,
      d: { "srv-x|general": 5 },
      c: "00000000",
    }));
    vi.resetModules();
    const reloaded = await import("./partyUnread.js");
    const s = { id: "srv-x", channels: [{ id: "general", messages: 5 }], members: [] };
    // Rejected, so everything reads as unread — the safe direction. Trusting it
    // would let an edited file hide messages that were never read.
    expect(reloaded.computeUnread([s]).total).toBe(5);
  });
});

describe("pruneTo", () => {
  it("drops marks for servers the user has left", () => {
    const kept = sid();
    const left = sid();
    markRead(kept, "general", 5);
    markRead(left, "general", 5);
    pruneTo([server(kept, 5)]);
    expect(computeUnread([server(kept, 5)]).total).toBe(0);
    // The departed server's mark is gone, so its threads read as fully unread.
    expect(computeUnread([server(left, 5)]).total).toBe(5);
  });
});
