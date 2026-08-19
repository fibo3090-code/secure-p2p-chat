// @vitest-environment jsdom
//
// The shell. What it owns, and what these tests pin: the gate that keeps every
// surface behind unlock, the boot screen that must never be a blank window, the
// presence signal the backend needs to tell a background message from one on
// screen, and the landmarks/skip links a keyboard or screen-reader user needs to
// reach the composer without crossing twenty controls.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("./lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api, onBridge: async () => () => {} };
});

import App from "./App.jsx";
import { stubApi } from "./test/render.jsx";

function useApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

const ready = {
  state: "ready", name: "Maya", min_password_len: 12, error: null,
  fingerprint: "ab".repeat(32),
};

const conv = {
  id: "chat-1", title: "Alice", last: "hello", connected: true, placeholder: false,
  verified: true, messages: 2, unread: 0, last_at: new Date().toISOString(),
  kind: "dm", transport: "direct",
};

function readyApi(over = {}) {
  return useApi({
    authStatus: ready,
    listConversations: [conv],
    listTransfers: [],
    pendingFingerprint: null,
    lockState: false,
    getConversation: {
      id: "chat-1", title: "Alice", peer_fingerprint: "ab".repeat(32), participants: [],
      created_at: new Date().toISOString(), is_host_placeholder: false,
      messages: [{ id: "m1", from_me: false, content: { type: "text", text: "hello" }, timestamp: new Date().toISOString() }],
      kind: "dm", transport: "direct",
    },
    partyList: [],
    ...over,
  });
}

beforeEach(() => { readyApi(); });

describe("App — boot", () => {
  it("shows a boot screen rather than a blank window while auth_status is pending", async () => {
    let resolve;
    useApi({ authStatus: () => new Promise((r) => { resolve = r; }) });
    const { container } = render(<App />);
    expect(container.querySelector(".onb-card")).toBeTruthy();
    resolve(ready);
    await screen.findByRole("navigation", { name: "Primary" });
  });

  it("reports a failed handshake and keeps retrying", async () => {
    useApi({ authStatus: async () => { throw new Error("the backend did not respond"); } });
    render(<App />);
    expect(await screen.findByText(/did not respond/)).toBeTruthy();
  });

  it("refuses to continue past an unreadable identity, and offers no retry", async () => {
    // Regenerating here would make every stored message undecryptable and break
    // TOFU with every contact, while looking like a fresh install.
    useApi({ authStatus: { ...ready, state: "error", error: "identity.json exists but could not be read" } });
    render(<App />);
    expect(await screen.findByText(/Nothing has been changed or deleted/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  it("blocks every surface behind the lock screen", async () => {
    useApi({ authStatus: { ...ready, state: "unlock" } });
    render(<App />);
    await screen.findByRole("button", { name: /unlock/i });
    expect(screen.queryByRole("navigation")).toBeNull();
    expect(screen.queryByRole("listbox", { name: "Conversations" })).toBeNull();
  });

  it("blocks every surface behind the set-password screen", async () => {
    useApi({ authStatus: { ...ready, state: "set_password" } });
    render(<App />);
    await screen.findByRole("button", { name: /Create identity/ });
    expect(screen.queryByRole("navigation")).toBeNull();
  });
});

describe("App — landmarks and keyboard access", () => {
  it("names its primary navigation", async () => {
    render(<App />);
    expect(await screen.findByRole("navigation", { name: "Primary" })).toBeTruthy();
  });

  it("puts skip links first in the tab order and points them at real targets", async () => {
    render(<App />);
    await screen.findByRole("navigation", { name: "Primary" });

    const links = screen.getAllByRole("link");
    expect(links[0].textContent).toBe("Skip to main content");
    expect(links[0].getAttribute("href")).toBe("#main-content");
    // The target has to exist, or the link is decoration.
    expect(document.querySelector("#main-content")).toBeTruthy();
    // And it must be focusable, or focus stays on the link.
    expect(document.querySelector("#main-content").getAttribute("tabindex")).toBe("-1");
  });

  it("offers a direct jump to the message box once a conversation is open", async () => {
    render(<App />);
    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));

    const jump = await screen.findByRole("link", { name: "Skip to message box" });
    expect(jump.getAttribute("href")).toBe("#composer-input");
    await waitFor(() => expect(document.querySelector("#composer-input")).toBeTruthy());
  });

  it("announces the unread count on a rail button instead of drawing a bare number", async () => {
    readyApi({ listConversations: [{ ...conv, unread: 4 }] });
    render(<App />);
    expect(await screen.findByRole("button", { name: "Chats, 4 unread" })).toBeTruthy();
  });

  it("marks the open section as the current page", async () => {
    render(<App />);
    const chats = await screen.findByRole("button", { name: /^Chats/ });
    expect(chats.getAttribute("aria-current")).toBe("page");
    await userEvent.click(screen.getByRole("button", { name: "Contacts" }));
    expect(screen.getByRole("button", { name: "Contacts" }).getAttribute("aria-current")).toBe("page");
  });
});

describe("App — presence and unread", () => {
  it("tells the bridge what is on screen, so a background message can be told apart", async () => {
    const api = readyApi();
    render(<App />);
    await waitFor(() => expect(api.setPresence).toHaveBeenCalled());
    // ChatManager owns no window handle: without this it cannot tell a message
    // arriving in the background from one the user is reading right now.
    expect(api.setPresence.mock.calls[0][1]).toBeNull();

    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));
    await waitFor(() =>
      expect(api.setPresence.mock.calls.at(-1)[1]).toBe("chat-1"));
  });

  it("clears the read mark for the conversation actually on screen", async () => {
    const api = readyApi();
    render(<App />);
    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));
    await waitFor(() => expect(api.markRead).toHaveBeenCalledWith("chat-1"));
  });
});

describe("App — navigation", () => {
  it("switches panes without losing the shell", async () => {
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: "Settings" }));
    expect(await screen.findByText(/identity · privacy · hosting/)).toBeTruthy();
    expect(screen.getByRole("navigation", { name: "Primary" })).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Relays" }));
    expect(await screen.findByText(/blind broker/)).toBeTruthy();
  });
});
