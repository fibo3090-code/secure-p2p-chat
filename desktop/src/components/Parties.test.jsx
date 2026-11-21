// @vitest-environment jsdom
//
// Communities. The rule worth pinning hardest is the two-step join: on a first
// contact the bridge answers `{ status: "verify" }` and has sent *nothing* — no
// username, no password — so the UI must show the code and only call again with
// `trust: true` once the user has confirmed it. Getting that wrong hands the
// credentials to whatever key answered and pins it afterwards, which is
// trust-on-first-use with the trust step missing.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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

function installApi(overrides) {
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
  installApi({ partyList: [], partySaved: [] });
});

describe("Parties — joining", () => {
  it("offers the join form when no community has been joined", async () => {
    render(<Parties />);
    expect(await screen.findByText("Join a community")).toBeTruthy();
  });

  it("refuses to submit without an address or a username", async () => {
    const api = installApi({ partyList: [], partySaved: [] });
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
    const api = installApi({ partyList: [], partySaved: [] });
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

  it("stops on a first join to show the fingerprint, and binds the join to it", async () => {
    const fp = "0123abcd".repeat(8);
    const partyJoin = vi.fn(async (_a, _u, _p, fingerprint) =>
      fingerprint
        ? { status: "joined", server: "s1", fingerprint }
        : { status: "verify", fingerprint: fp, sas: "418 902 🎃🎈🎁" });
    installApi({ partyList: [], partySaved: [], partyJoin });

    render(<Parties />);
    await screen.findByText("Join a community");
    await userEvent.type(screen.getByPlaceholderText(/server address/), "10.0.0.5:12345");
    await userEvent.type(screen.getByPlaceholderText("username"), "me");
    await userEvent.click(screen.getByRole("button", { name: /Connect & join/ }));

    expect(await screen.findByText("Verify this community server")).toBeTruthy();
    // What the operator can actually compare: the server's fingerprint, which
    // it prints at startup. A per-connection code is never shown — the
    // operator cannot see it, and it belongs to a connection that is closed.
    expect(screen.getByText(fp.match(/.{8}/g).join(" "))).toBeTruthy();
    expect(screen.queryByText("418 902 🎃🎈🎁")).toBeNull();
    // The sentence is split by a <strong>, so match on the rendered text.
    expect(document.body.textContent).toMatch(/have not been sent yet/);
    expect(document.body.textContent).toMatch(/not end-to-end encrypted/);
    expect(partyJoin).toHaveBeenCalledTimes(1);
    expect(partyJoin.mock.calls[0][3]).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /The fingerprint matches/ }));
    await waitFor(() => expect(partyJoin).toHaveBeenCalledTimes(2));
    // Step two carries exactly the verified fingerprint, so the connection
    // that sends the password must present that key.
    expect(partyJoin.mock.calls[1][3]).toBe(fp);
  });

  it("backs out of verification without joining", async () => {
    const partyJoin = vi.fn(async () => ({ status: "verify", fingerprint: "aa".repeat(32), sas: "1 2 3" }));
    installApi({ partyList: [], partySaved: [], partyJoin });
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
    installApi({
      partyList: [],
      partySaved: async () => { throw new Error("parties.json is damaged"); },
    });
    render(<Parties />);
    expect(await screen.findByText(/parties.json is damaged/)).toBeTruthy();
  });

  it("fills the form from a saved community but does not join from the card", async () => {
    const api = installApi({
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
    installApi({
      partyList: [joinedServer],
      partySaved: [],
      partyHistory: [{ sender_name: "nova", from_me: false, kind: "text", text: "welcome", size: null, timestamp: Date.now() }],
    });
    render(<Parties />);
    expect(await screen.findByText("welcome")).toBeTruthy();
  });

  // UI-03: one draft per thread. A shared draft followed the user from a
  // private DM into a public channel, and Enter posted it to everyone.
  it("keeps a DM's unsent text in the DM, never in a channel", async () => {
    const api = installApi({
      partyList: [joinedServer], partySaved: [], partyHistory: [], partyDmHistory: [],
    });
    render(<Parties />);
    await screen.findByLabelText(/^Message # general/);
    await userEvent.click(screen.getByTitle("Direct message nova"));
    const dmBox = await screen.findByLabelText(/^Message ✉ nova/);
    await userEvent.type(dmBox, "only for nova");

    await userEvent.click(screen.getByTitle("#general"));
    const channelBox = await screen.findByLabelText(/^Message # general/);
    expect(channelBox.value).toBe("");
    await userEvent.type(channelBox, "{Enter}");
    expect(api.partyPost).not.toHaveBeenCalled();

    await userEvent.click(screen.getByTitle("Direct message nova"));
    expect((await screen.findByLabelText(/^Message ✉ nova/)).value).toBe("only for nova");
  });

  // UI-04: every selection change and every `party-updated` tick starts a
  // history fetch, and a slower one for the channel just left could resolve
  // last — rendering one thread's messages (a private channel's, say) under
  // another thread's name until the next event.
  it("never shows a thread's history under another thread's header", async () => {
    const twoChannels = {
      ...joinedServer,
      channels: [
        ...joinedServer.channels,
        { id: "ch2", name: "secret", messages: 1, kind: "private", members: [], can_post: true },
      ],
    };
    const pending = {};
    installApi({
      partyList: [twoChannels],
      partySaved: [],
      partyHistory: (_sid, cid) => new Promise((resolve) => { (pending[cid] ||= []).push(resolve); }),
    });
    const msg = (text) => [{ sender_name: "nova", from_me: false, kind: "text", text, size: null, timestamp: Date.now() }];

    render(<Parties />);
    await waitFor(() => expect(pending.ch1?.length).toBeGreaterThan(0));
    await userEvent.click(screen.getByTitle("#secret — private"));
    await waitFor(() => expect(pending.ch2?.length).toBeGreaterThan(0));

    // The new channel answers first, the one that was left answers last.
    pending.ch2.forEach((r) => r(msg("in secret")));
    expect(await screen.findByText("in secret")).toBeTruthy();
    pending.ch1.forEach((r) => r(msg("in general")));
    await new Promise((r) => setTimeout(r, 20));

    expect(screen.queryByText("in general")).toBeNull();
    expect(screen.getByText("in secret")).toBeTruthy();
  });

  // JOURNEY-04 / PRIV-01: the operator reads community traffic, so the
  // conversation itself has to say so — not only the join screen.
  it("says on screen that the operator can read the community", async () => {
    installApi({ partyList: [joinedServer], partySaved: [], partyHistory: [] });
    render(<Parties />);
    expect(await screen.findByText("Operator can read messages")).toBeTruthy();
  });

  it("posts to the active channel and clears the composer", async () => {
    const api = installApi({ partyList: [joinedServer], partySaved: [], partyHistory: [] });
    render(<Parties />);
    const box = await screen.findByLabelText(/^Message/);
    await userEvent.type(box, "hello");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(api.partyPost).toHaveBeenCalledWith("s1", "ch1", "hello"));
    expect(box.value).toBe("");
  });

  it("puts a refused post back in the composer instead of losing it", async () => {
    installApi({
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
    installApi({ partyList: [locked], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await waitFor(() => expect(composer()?.getAttribute("placeholder")).toMatch(/Only admins can post/));
    expect(composer().disabled).toBe(true);
  });

  it("explains a guest's read-only role", async () => {
    installApi({ partyList: [{ ...joinedServer, my_role: "guest" }], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await waitFor(() => expect(composer()?.getAttribute("placeholder")).toMatch(/read-only/));
    expect(composer().disabled).toBe(true);
  });
});

describe("Parties — managing a community", () => {
  // Leaving forgets the server's pinned fingerprint, so rejoining is an
  // unverified first contact again: one click must not do it.
  it("leaves only on a second click, and says what the second click costs", async () => {
    const api = installApi({ partyList: [joinedServer], partySaved: [], partyHistory: [] });
    render(<Parties />);
    const leave = await screen.findByTitle("Leave this community");
    await userEvent.click(leave);
    expect(api.partyLeave).not.toHaveBeenCalled();
    expect(leave.getAttribute("title")).toMatch(/forgets the server's verified fingerprint/);
    await userEvent.click(leave);
    await waitFor(() => expect(api.partyLeave).toHaveBeenCalledWith("s1"));
  });

  it("deletes a channel and its history only on a second click", async () => {
    const api = installApi({ partyList: [joinedServer], partySaved: [], partyHistory: [] });
    render(<Parties />);
    const del = await screen.findByTitle("Delete #general");
    await userEvent.click(del);
    expect(api.partyDeleteChannel).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTitle(/Click again to delete #general/));
    await waitFor(() => expect(api.partyDeleteChannel).toHaveBeenCalledWith("s1", "ch1"));
  });

  it("creates a channel from the box", async () => {
    const api = installApi({ partyList: [joinedServer], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await userEvent.type(await screen.findByPlaceholderText("new channel"), "ops{Enter}");
    await waitFor(() => expect(api.partyCreateChannel).toHaveBeenCalledWith("s1", "ops"));
  });

  // A message card names its file in `text`; reading only `name` saved
  // every download as "undefined". The click must also visibly work, and a
  // second click while it runs must not queue another request.
  it("downloads a file under its own name, once, showing that it is working", async () => {
    let finish;
    const partyDownloadFile = vi.fn(() => new Promise((r) => { finish = r; }));
    installApi({
      partyList: [joinedServer], partySaved: [],
      partyHistory: [{ sender_name: "nova", from_me: false, kind: "file", text: "report.pdf",
        size: 2048, hash: "ab".repeat(32), timestamp: Date.now() }],
      partyDownloadFile,
    });
    render(<Parties />);
    await userEvent.click(await screen.findByTitle("Download report.pdf"));
    expect(partyDownloadFile).toHaveBeenCalledWith("s1", "ab".repeat(32), "report.pdf");
    const busy = await screen.findByTitle("Downloading…");
    expect(busy.disabled).toBe(true);
    await userEvent.click(busy);
    expect(partyDownloadFile).toHaveBeenCalledTimes(1);
    finish();
    expect(await screen.findByTitle("Download report.pdf")).toBeTruthy();
  });

  it("explains a rejected join and offers to rejoin with the address filled in", async () => {
    installApi({
      partyList: [{ ...joinedServer, status: "rejected", status_detail: "wrong password" }],
      partySaved: [], partyHistory: [],
    });
    render(<Parties />);
    expect((await screen.findAllByText(/Join rejected: wrong password/)).length).toBeGreaterThan(0);
    await userEvent.click(screen.getByRole("button", { name: /Rejoin/ }));
    expect(await screen.findByDisplayValue("10.0.0.5:12345")).toBeTruthy();
    expect(screen.getByDisplayValue("you")).toBeTruthy();
  });
});

// The dialogs are tested on their own in PartyAdmin.test.jsx; what is pinned
// here is the wiring — which bridge command each one ends in, with which
// target. Mixing those up is silent: "edit #general" that calls create makes a
// second channel, and a delete with the wrong location removes a different
// reference to the same content-addressed file.
describe("Parties — governance wiring", () => {
  const file = {
    hash: "h1", name: "roadmap.pdf", size: 10, uploader: "m2", uploader_name: "nova",
    // Not the channel that opens first: an action that used the open channel
    // instead of the file's own location would otherwise pass.
    location: "ch2", location_name: "#random", is_dm: false, shared_at: 1700000000000,
    can_view: true, can_download: true, can_share: true, can_delete: true,
  };
  const withMembers = (my_role) => ({
    ...joinedServer, my_role,
    channels: [...joinedServer.channels,
      { id: "ch2", name: "random", messages: 0, kind: "public", members: [], can_post: true }],
    members: [
      { id: "me", username: "you", online: true, is_me: true, dm_messages: 0, role: my_role },
      { id: "o", username: "olga", online: true, is_me: false, dm_messages: 0, role: "owner" },
      { id: "a", username: "ada", online: true, is_me: false, dm_messages: 0, role: "admin" },
      { id: "m2", username: "nova", online: true, is_me: false, dm_messages: 0, role: "member" },
      { id: "g", username: "gus", online: true, is_me: false, dm_messages: 0, role: "guest" },
    ],
    files: [file],
  });
  const roleOptions = (name) => {
    const sel = screen.queryByTitle(`Role for ${name}`);
    return sel ? [...sel.querySelectorAll("option")].map((o) => o.value) : null;
  };

  // The server refuses regardless, but a control that cannot work is a
  // control that reads as broken. Offered: only below your own level, never
  // on yourself, never on the owner.
  it("offers a role control only where the server would accept the change", async () => {
    installApi({ partyList: [withMembers("owner")], partySaved: [], partyHistory: [] });
    const { unmount } = render(<Parties />);
    await screen.findByTitle("Role for nova");
    expect(roleOptions("you")).toBeNull();
    expect(roleOptions("olga")).toBeNull();
    expect(roleOptions("ada")).toEqual(["guest", "member", "admin"]);
    expect(roleOptions("nova")).toEqual(["guest", "member", "admin"]);
    unmount();

    _resetForTests();
    installApi({ partyList: [withMembers("admin")], partySaved: [], partyHistory: [] });
    const second = render(<Parties />);
    await screen.findByTitle("Role for nova");
    expect(roleOptions("ada")).toBeNull();
    expect(roleOptions("olga")).toBeNull();
    expect(roleOptions("nova")).toEqual(["guest", "member"]);
    expect(roleOptions("gus")).toEqual(["guest", "member"]);
    second.unmount();

    _resetForTests();
    installApi({ partyList: [withMembers("member")], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await screen.findByText("nova");
    expect(document.querySelector(".party-rolesel")).toBeNull();
  });

  it("a role change goes to the member it was made on", async () => {
    const api = installApi({ partyList: [withMembers("owner")], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await userEvent.selectOptions(await screen.findByTitle("Role for nova"), "admin");
    await waitFor(() => expect(api.partySetRole).toHaveBeenCalledWith("s1", "m2", "admin"));
  });

  it("editing a channel's access changes that channel; a new one is created", async () => {
    const api = installApi({ partyList: [withMembers("owner")], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await userEvent.click(await screen.findByTitle("Change who can use #general"));
    let dialog = await screen.findByRole("dialog", { name: "Access for #general" });
    await userEvent.click(within(dialog).getByRole("button", { name: /Private/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: /nova/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(api.partySetChannelAccess).toHaveBeenCalledWith("s1", "ch1", "private", ["m2"]));
    expect(api.partyCreateChannelKind).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await userEvent.click(screen.getByTitle("New channel with a specific kind"));
    dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByPlaceholderText("announcements"), "news");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create channel" }));
    await waitFor(() =>
      expect(api.partyCreateChannelKind).toHaveBeenCalledWith("s1", "news", "public", []));
    expect(api.partySetChannelAccess).toHaveBeenCalledTimes(1);
  });

  it("drive actions act on the file's own location, then refresh the listing", async () => {
    const api = installApi({ partyList: [withMembers("owner")], partySaved: [], partyHistory: [] });
    render(<Parties />);
    await userEvent.click(await screen.findByTitle("Shared files"));
    // Opening the drive refreshes it too, so count what each action adds.
    await screen.findByTitle(/^Share roadmap.pdf/);
    const refreshes = () => api.partyRefreshFiles.mock.calls.length;
    let before = refreshes();

    await userEvent.click(screen.getByTitle(/^Share roadmap.pdf/));
    let dialog = await screen.findByRole("dialog", { name: "Share roadmap.pdf" });
    await userEvent.click(within(dialog).getByRole("button", { name: /general/ }));
    await waitFor(() =>
      expect(api.partyShareFile).toHaveBeenCalledWith("s1", "h1", "ch2", { channel: "ch1" }));
    await waitFor(() => expect(refreshes()).toBe(before + 1));
    before = refreshes();

    await userEvent.click(screen.getByTitle("Choose who can use roadmap.pdf"));
    dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: /nova/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(api.partySetFilePermissions).toHaveBeenCalledTimes(1));
    const [srv, hash, loc, member, perms] = api.partySetFilePermissions.mock.calls[0];
    expect([srv, hash, loc, member]).toEqual(["s1", "h1", "ch2", "m2"]);
    expect(perms).toMatchObject({ view: true, download: true });
    await waitFor(() => expect(refreshes()).toBe(before + 1));
    before = refreshes();

    await userEvent.click(screen.getByTitle("Delete this file"));
    await userEvent.click(screen.getByTitle(/Click again to delete/));
    await waitFor(() => expect(api.partyDeleteFile).toHaveBeenCalledWith("s1", "h1", "ch2"));
    await waitFor(() => expect(refreshes()).toBe(before + 1));
  });

  it("a refused governance action says why", async () => {
    installApi({
      partyList: [withMembers("owner")], partySaved: [], partyHistory: [],
      partySetRole: vi.fn(async () => { throw "only the owner can do that"; }),
    });
    render(<Parties />);
    await userEvent.selectOptions(await screen.findByTitle("Role for nova"), "guest");
    const { subscribe } = await import("../lib/toast.js");
    await waitFor(() => {
      let shown = [];
      subscribe((l) => { shown = l; })();
      expect(shown.some((t) => t.level === "error" && /only the owner/.test(t.message))).toBe(true);
    });
  });

  it("an error from the server can be dismissed", async () => {
    const api = installApi({
      partyList: [{ ...joinedServer, last_error: "upload refused: quota" }],
      partySaved: [], partyHistory: [],
    });
    render(<Parties />);
    expect(await screen.findByText("upload refused: quota")).toBeTruthy();
    await userEvent.click(screen.getByTitle("Dismiss"));
    await waitFor(() => expect(api.partyClearError).toHaveBeenCalledWith("s1"));
  });

  // Removing a dead entry discards its pinned fingerprint, exactly like Leave.
  it("removes a dead community only on a second click", async () => {
    const api = installApi({
      partyList: [{ ...joinedServer, status: "disconnected" }],
      partySaved: [], partyHistory: [],
    });
    render(<Parties />);
    const remove = await screen.findByRole("button", { name: "Remove" });
    await userEvent.click(remove);
    expect(api.partyLeave).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(api.partyLeave).toHaveBeenCalledWith("s1"));
  });
});
