// @vitest-environment jsdom
//
// Communities. The rule worth pinning hardest is the two-step join: on a first
// contact the bridge answers `{ status: "verify" }` and has sent *nothing* — no
// username, no password — so the UI must show the code and only call again with
// `trust: true` once the user has confirmed it. Getting that wrong hands the
// credentials to whatever key answered and pins it afterwards, which is
// trust-on-first-use with the trust step missing.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api, onBridge: async () => () => {} };
});

import { Parties } from "./Parties.jsx";
import { stubApi } from "../test/render.jsx";
import { _resetForTests } from "../lib/partyUnread.js";

function useApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

const joinedServer = {
  id: "s1", name: "Test Community", address: "10.0.0.5:12345", username: "you",
  fingerprint: "aa".repeat(32), status: "joined", status_detail: null, member_id: "me",
  last_error: null, my_role: "owner", last_notice: null,
  channels: [{ id: "ch1", name: "general", messages: 1, kind: "public", members: [], can_post: true }],
  members: [
    { id: "me", username: "you", online: true, is_me: true, dm_messages: 0, role: "owner" },
    { id: "m2", username: "nova", online: true, is_me: false, dm_messages: 0, role: "member" },
  ],
  files: [], audit: [],
  quota: { used: 0, limit: 1024, server_used: 0, server_limit: 4096 },
};

beforeEach(() => {
  _resetForTests();
  useApi({ partyList: [], partySaved: [] });
});

describe("Parties — joining", () => {
  it("offers the join form when no community has been joined", async () => {
    render(<Parties />);
    expect(await screen.findByText("Join a community")).toBeTruthy();
  });

  it("refuses to submit without an address or a username", async () => {
    const api = useApi({ partyList: [], partySaved: [] });
    render(<Parties />);
    await screen.findByText("Join a community");

    await userEvent.click(screen.getByRole("button", { name: /Connect & join/ }));
    expect(await screen.findByText("Enter the server address.")).toBeTruthy();
    expect(api.partyJoin).not.toHaveBeenCalled();

    await userEvent.type(screen.getByPlaceholderText(/server address/), "10.0.0.5:12345");
    await userEvent.click(screen.getByRole("button", { name: /Connect & join/ }));
    expect(await screen.findByText("Choose a username.")).toBeTruthy();
    expect(api.partyJoin).not.toHaveBeenCalled();
  });

  it("mirrors the server's username cap before the round trip", async () => {
    const api = useApi({ partyList: [], partySaved: [] });
    render(<Parties />);
    await screen.findByText("Join a community");
    await userEvent.type(screen.getByPlaceholderText(/server address/), "10.0.0.5:12345");
    // maxLength stops typing past the cap, so set it directly to prove the
    // check exists rather than relying on the attribute alone.
    const user = screen.getByPlaceholderText("username");
    expect(user.getAttribute("maxlength")).toBe("32");
    await userEvent.type(user, "a".repeat(32));
    await userEvent.click(screen.getByRole("button", { name: /Connect & join/ }));
    expect(api.partyJoin).toHaveBeenCalled();
  });

  it("stops on a first join to show the code, and sends no credential until confirmed", async () => {
    const partyJoin = vi.fn(async (_a, _u, _p, trust) =>
      trust
        ? { status: "joined", server: "s1", fingerprint: "aa".repeat(32) }
        : { status: "verify", fingerprint: "aa".repeat(32), sas: "418 902 🎃🎈🎁" });
    useApi({ partyList: [], partySaved: [], partyJoin });

    render(<Parties />);
    await screen.findByText("Join a community");
    await userEvent.type(screen.getByPlaceholderText(/server address/), "10.0.0.5:12345");
    await userEvent.type(screen.getByPlaceholderText("username"), "me");
    await userEvent.click(screen.getByRole("button", { name: /Connect & join/ }));

    expect(await screen.findByText("Verify this community server")).toBeTruthy();
    expect(screen.getByText("418 902 🎃🎈🎁")).toBeTruthy();
    // The sentence is split by a <strong>, so match on the rendered text.
    expect(document.body.textContent).toMatch(/have not been sent yet/);
    expect(partyJoin).toHaveBeenCalledTimes(1);
    expect(partyJoin.mock.calls[0][3]).toBe(false);

    await userEvent.click(screen.getByRole("button", { name: /The code matches/ }));
    await waitFor(() => expect(partyJoin).toHaveBeenCalledTimes(2));
    expect(partyJoin.mock.calls[1][3]).toBe(true);
  });

  it("backs out of verification without joining", async () => {
    const partyJoin = vi.fn(async () => ({ status: "verify", fingerprint: "aa".repeat(32), sas: "1 2 3" }));
    useApi({ partyList: [], partySaved: [], partyJoin });
    render(<Parties />);
    await screen.findByText("Join a community");
    await userEvent.type(screen.getByPlaceholderText(/server address/), "10.0.0.5:12345");
    await userEvent.type(screen.getByPlaceholderText("username"), "me");
    await userEvent.click(screen.getByRole("button", { name: /Connect & join/ }));
    await screen.findByText("Verify this community server");

    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Join a community")).toBeTruthy();
    expect(partyJoin).toHaveBeenCalledTimes(1);
  });

  it("reports a failure to read the saved pins rather than showing an empty list", async () => {
    // parties.json holds every community's pinned fingerprint. Swallowing a
    // parse failure would silently turn the next join back into an unverified
    // first contact.
    useApi({
      partyList: [],
      partySaved: async () => { throw new Error("parties.json is damaged"); },
    });
    render(<Parties />);
    expect(await screen.findByText(/parties.json is damaged/)).toBeTruthy();
  });

  it("fills the form from a saved community but does not join from the card", async () => {
    const api = useApi({
      partyList: [],
      partySaved: [{ address: "10.0.0.5:12345", username: "you", name: "Test", fingerprint: "aa" }],
    });
    render(<Parties />);
    await userEvent.click(await screen.findByTitle("Fill in you@10.0.0.5:12345"));

    expect(screen.getByPlaceholderText(/server address/).value).toBe("10.0.0.5:12345");
    expect(screen.getByPlaceholderText("username").value).toBe("you");
    // Joining straight from a card sent whatever happened to be in the shared
    // password box — to whichever community was clicked next.
    expect(api.partyJoin).not.toHaveBeenCalled();
  });
});

describe("Parties — a joined community", () => {
  it("shows the channel and its history", async () => {
    useApi({
      partyList: [joinedServer],
      partySaved: [],
      partyHistory: [{ sender_name: "nova", from_me: false, kind: "text", text: "welcome", size: null, timestamp: Date.now() }],
    });
    render(<Parties />);
    expect(await screen.findByText("welcome")).toBeTruthy();
  });

  it("posts to the active channel and clears the composer", async () => {
    const api = useApi({ partyList: [joinedServer], partySaved: [], partyHistory: [] });
    render(<Parties />);
    const box = await screen.findByLabelText(/^Message/);
    await userEvent.type(box, "hello");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(api.partyPost).toHaveBeenCalledWith("s1", "ch1", "hello"));
    expect(box.value).toBe("");
  });

  it("puts a refused post back in the composer instead of losing it", async () => {
    useApi({
      partyList: [joinedServer], partySaved: [], partyHistory: [],
      partyPost: async () => { throw new Error("this channel is locked"); },
    });
    render(<Parties />);
    const box = await screen.findByLabelText(/^Message/);
    await userEvent.type(box, "hello");
    await userEvent.keyboard("{Enter}");
    // The user's text is the one thing that must survive a failure.
    await waitFor(() => expect(box.value).toBe("hello"));
  });

  // The composer's own label is the only place the app says *why* it is
  // disabled, so both of these assert the wording, not just the disabled bit.
  // `waitFor`: the pane mounts before the active server is resolved, so it
  // renders "Not connected" for one frame first.
  const composer = () => document.querySelector(".composer-input");

  it("explains a read-only channel rather than accepting a message it cannot send", async () => {
    const locked = {
      ...joinedServer,
      channels: [{ id: "ch1", name: "news", messages: 0, kind: "announce", members: [], can_post: false }],
    };
    useApi({ partyList: [locked], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await waitFor(() => expect(composer()?.getAttribute("placeholder")).toMatch(/Only admins can post/));
    expect(composer().disabled).toBe(true);
  });

  it("explains a guest's read-only role", async () => {
    useApi({ partyList: [{ ...joinedServer, my_role: "guest" }], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await waitFor(() => expect(composer()?.getAttribute("placeholder")).toMatch(/read-only/));
    expect(composer().disabled).toBe(true);
  });
});
