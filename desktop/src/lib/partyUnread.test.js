import { describe, it, expect } from "vitest";
import { computeUnread, markRead } from "./partyUnread.js";

// The module keeps one process-wide store, so every test uses its own server
// id to stay independent of the others.
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

describe("computeUnread", () => {
  it("seeds the baseline on first sighting so old history is not unread", () => {
    const id = sid();
    const { total, byKey } = computeUnread([server(id, 40)]);
    expect(total).toBe(0);
    expect(byKey).toEqual({});
  });

  it("counts growth beyond the baseline", () => {
    const id = sid();
    computeUnread([server(id, 10)]);
    const { total, byKey } = computeUnread([server(id, 13)]);
    expect(total).toBe(3);
    expect(byKey[`${id}|general`]).toBe(3);
  });

  it("never reports negative unread when counts shrink", () => {
    const id = sid();
    computeUnread([server(id, 10)]);
    const { total } = computeUnread([server(id, 4)]);
    expect(total).toBe(0);
  });

  it("tracks DM threads but skips the local member", () => {
    const id = sid();
    computeUnread([server(id, 0, 5)]);
    const { total, byKey } = computeUnread([server(id, 0, 7)]);
    expect(total).toBe(2);
    expect(byKey[`${id}|dm-peer`]).toBe(2);
    expect(Object.keys(byKey)).not.toContain(`${id}|dm-me`);
  });

  it("markRead clears a thread's unread count", () => {
    const id = sid();
    computeUnread([server(id, 10)]);
    computeUnread([server(id, 15)]);
    markRead(id, "general", 15);
    const { total } = computeUnread([server(id, 15)]);
    expect(total).toBe(0);
  });
});
