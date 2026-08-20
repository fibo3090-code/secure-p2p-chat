// @vitest-environment jsdom
//
// The "New connection" dialog. It stays mounted between opens, which is where
// its sharpest bug lived: a connection password typed for host A survived into
// the next open and was silently sent to host B. The other rule pinned here is
// that a typo'd port is reported rather than quietly replaced with 12345 —
// dialling a different machine's port than the one on screen produces a plain
// "connection refused" and no way to work out why.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { Creator } from "./Creator.jsx";
import { stubApi } from "../test/render.jsx";
import { subscribe, dismiss } from "../lib/toast.js";

function installApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}
function toasts() {
  let current = [];
  subscribe((l) => { current = l; })();
  return current;
}
function clearToasts() { toasts().forEach((t) => dismiss(t.id)); }

beforeEach(() => {
  clearToasts();
  installApi({ myInviteLink: "chat-p2p://invite/abc", listDiscoveredPeers: { enabled: false, peers: [] } });
});
afterEach(clearToasts);

const open = (props = {}) =>
  render(<Creator open onClose={() => {}} initialMode="connect" {...props} />);

// "Connect" names both the mode tab and the action button, so the action is
// always looked up inside the pane rather than across the whole dialog.
const inPane = () => within(document.querySelector(".creator-pane"));

describe("Creator — dialling a peer", () => {
  it("refuses an empty address", async () => {
    const api = installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    open();
    await userEvent.click(inPane().getByRole("button", { name: "Connect" }));
    expect(screen.getByText("Enter a peer address.")).toBeTruthy();
    expect(api.connectPeer).not.toHaveBeenCalled();
  });

  it("names a bad port instead of silently dialling 12345", async () => {
    const api = installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    open();
    await userEvent.type(screen.getByPlaceholderText("192.168.1.20:12345"), "10.0.0.5:99999");
    await userEvent.click(inPane().getByRole("button", { name: "Connect" }));
    expect(screen.getByText(/"99999" is not a valid port/)).toBeTruthy();
    expect(api.connectPeer).not.toHaveBeenCalled();
  });

  it("defaults the port only when none was typed at all", async () => {
    const api = installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    open();
    await userEvent.type(screen.getByPlaceholderText("192.168.1.20:12345"), "10.0.0.5");
    await userEvent.click(inPane().getByRole("button", { name: "Connect" }));
    expect(api.connectPeer).toHaveBeenCalledWith("10.0.0.5", 12345, "");
  });

  it("splits host and port on the last colon, so IPv6 survives", async () => {
    const api = installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    open();
    // `[` and `{` are userEvent keyboard modifiers, so the address is typed
    // with them escaped.
    await userEvent.type(screen.getByPlaceholderText("192.168.1.20:12345"), "[[fe80::1]:9000");
    await userEvent.click(inPane().getByRole("button", { name: "Connect" }));
    expect(api.connectPeer).toHaveBeenCalledWith("[fe80::1]", 9000, "");
  });

  it("surfaces a refused connection in the dialog", async () => {
    installApi({
      listDiscoveredPeers: { enabled: false, peers: [] },
      connectPeer: async () => { throw new Error("connection refused"); },
    });
    open();
    await userEvent.type(screen.getByPlaceholderText("192.168.1.20:12345"), "10.0.0.5:9000");
    await userEvent.click(inPane().getByRole("button", { name: "Connect" }));
    expect(await screen.findByText(/connection refused/)).toBeTruthy();
  });

  it("clears the connection password when the dialog closes", async () => {
    const api = installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    const { rerender } = render(<Creator open onClose={() => {}} initialMode="connect" />);
    await userEvent.type(screen.getByPlaceholderText(/Connection password/), "hunter2hunter2");

    // Reopening must not carry host A's password over to host B.
    rerender(<Creator open={false} onClose={() => {}} initialMode="connect" />);
    rerender(<Creator open onClose={() => {}} initialMode="connect" />);

    await userEvent.type(screen.getByPlaceholderText("192.168.1.20:12345"), "10.0.0.9:9000");
    await userEvent.click(inPane().getByRole("button", { name: "Connect" }));
    expect(api.connectPeer).toHaveBeenCalledWith("10.0.0.9", 9000, "");
  });
});

describe("Creator — hosting", () => {
  it("rejects a bad listening port", async () => {
    const api = installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    open({ initialMode: "host" });
    const port = screen.getByDisplayValue("12345");
    await userEvent.clear(port);
    await userEvent.type(port, "0");
    await userEvent.click(screen.getByRole("button", { name: /Start hosting/ }));
    expect(screen.getByText(/"0" is not a valid port/)).toBeTruthy();
    expect(api.startHost).not.toHaveBeenCalled();
  });

  it("starts hosting and shows the address to share", async () => {
    const api = installApi({
      listDiscoveredPeers: { enabled: false, peers: [] },
      myAddresses: { hosting: true, local: "192.168.1.9:12345", external: null },
    });
    open({ initialMode: "host" });
    await userEvent.click(screen.getByRole("button", { name: /Start hosting/ }));
    expect(api.startHost).toHaveBeenCalledWith(12345, "");
    expect(await screen.findByText("192.168.1.9:12345")).toBeTruthy();
  });
});

describe("Creator — invites", () => {
  it("shows your own invite link", async () => {
    open({ initialMode: "invite" });
    expect(await screen.findByText("chat-p2p://invite/abc")).toBeTruthy();
  });

  it("refuses an empty link", async () => {
    const api = installApi({ myInviteLink: "chat-p2p://invite/abc" });
    open({ initialMode: "invite" });
    await userEvent.click(screen.getByRole("button", { name: /Add & connect|Add/ }));
    expect(screen.getByText("Paste an invite link.")).toBeTruthy();
    expect(api.importInvite).not.toHaveBeenCalled();
  });

  it("connects the contact the import actually returned", async () => {
    // `import_invite` answers with { contact, signed }. Reading `.id` off the
    // wrapper called connect_contact with `undefined`, which failed to parse as
    // a UUID — so importing from here never connected, and said "Saved
    // undefined to contacts."
    const api = installApi({
      myInviteLink: "chat-p2p://invite/abc",
      importInvite: async () => ({ contact: { id: "c9", name: "Imported" }, signed: true }),
    });
    open({ initialMode: "invite" });
    await userEvent.type(screen.getByPlaceholderText(/chat-p2p:\/\/invite/i), "chat-p2p://invite/xyz");
    await userEvent.click(screen.getByRole("button", { name: /Add & connect|Add/ }));
    expect(api.importInvite).toHaveBeenCalledWith("chat-p2p://invite/xyz");
    expect(api.connectContact).toHaveBeenCalledWith("c9");
  });

  it("names the saved contact when the connect leg fails", async () => {
    installApi({
      myInviteLink: "chat-p2p://invite/abc",
      importInvite: async () => ({ contact: { id: "c9", name: "Imported" }, signed: true }),
      connectContact: async () => { throw new Error("no route"); },
    });
    open({ initialMode: "invite" });
    await userEvent.type(screen.getByPlaceholderText(/chat-p2p:\/\/invite/i), "chat-p2p://invite/xyz");
    await userEvent.click(screen.getByRole("button", { name: /Add & connect|Add/ }));
    expect(await screen.findByText(/Saved Imported to contacts/)).toBeTruthy();
  });

  it("warns that an unsigned link proves nothing about who made it", async () => {
    installApi({
      myInviteLink: "chat-p2p://invite/abc",
      importInvite: async () => ({ contact: { id: "c9", name: "Imported" }, signed: false }),
    });
    open({ initialMode: "invite" });
    await userEvent.type(screen.getByPlaceholderText(/chat-p2p:\/\/invite/i), "chat-p2p://invite/v1");
    await userEvent.click(screen.getByRole("button", { name: /Add & connect|Add/ }));
    expect(toasts().at(-1).message).toMatch(/unsigned invite link/);
  });
});

describe("Creator — nearby peers", () => {
  it("hides the nearby list entirely when mDNS is off", async () => {
    installApi({ listDiscoveredPeers: { enabled: false, peers: [] } });
    open();
    expect(screen.queryByText("Nearby peers")).toBeNull();
  });

  it("offers a discovered peer as a one-click address, never as trust", async () => {
    installApi({
      listDiscoveredPeers: {
        enabled: true,
        peers: [{ name: "laptop-alice", address: "192.168.1.21", port: 12345, fingerprint: "a1b2" }],
      },
    });
    open();
    const row = await screen.findByText("laptop-alice");
    await userEvent.click(row);
    // Discovery supplies the address only — TOFU still applies on connect.
    expect(screen.getByPlaceholderText("192.168.1.20:12345").value).toBe("192.168.1.21:12345");
  });
});
