// @vitest-environment jsdom
//
// The shell. What it owns, and what these tests pin: the gate that keeps every
// surface behind unlock, the boot screen that must never be a blank window, the
// presence signal the backend needs to tell a background message from one on
// screen, and the landmarks/skip links a keyboard or screen-reader user needs to
// reach the composer without crossing twenty controls.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("./lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api, onBridge: async () => () => {} };
});

import App from "./App.jsx";
import { stubApi } from "./test/render.jsx";

function installApi(overrides) {
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
  return installApi({
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
    installApi({ authStatus: () => new Promise((r) => { resolve = r; }) });
    const { container } = render(<App />);
    expect(container.querySelector(".onb-card")).toBeTruthy();
    resolve(ready);
    await screen.findByRole("navigation", { name: "Primary" });
  });

  it("reports a failed handshake and keeps retrying until the backend answers", async () => {
    // It used to assert only the error text, which passes for a shell that
    // gives up after one try. The retry is the point: the backend can be slow
    // to come up, and a window stuck on an error until restarted is broken.
    let calls = 0;
    const authStatus = vi.fn(async () => {
      calls += 1;
      if (calls === 1) throw new Error("the backend did not respond");
      return ready;
    });
    readyApi({ authStatus });
    render(<App />);
    expect(await screen.findByText(/did not respond/)).toBeTruthy();
    await screen.findByRole("navigation", { name: "Primary" }, { timeout: 3000 });
    expect(authStatus.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("refuses to continue past an unreadable identity, and offers no retry", async () => {
    // Regenerating here would make every stored message undecryptable and break
    // TOFU with every contact, while looking like a fresh install.
    installApi({ authStatus: { ...ready, state: "error", error: "identity.json exists but could not be read" } });
    render(<App />);
    expect(await screen.findByText(/Nothing has been changed or deleted/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
  });

  it("blocks every surface behind the lock screen", async () => {
    installApi({ authStatus: { ...ready, state: "unlock" } });
    render(<App />);
    await screen.findByRole("button", { name: /unlock/i });
    expect(screen.queryByRole("navigation")).toBeNull();
    expect(screen.queryByRole("listbox", { name: "Conversations" })).toBeNull();
  });

  it("blocks every surface behind the set-password screen", async () => {
    installApi({ authStatus: { ...ready, state: "set_password" } });
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

describe("App — conversation switching", () => {
  const bob = { ...conv, id: "chat-2", title: "Bob", last: "yo" };
  const detail = (id, title) => ({
    id, title, peer_fingerprint: "ab".repeat(32), participants: [],
    created_at: new Date().toISOString(), is_host_placeholder: false,
    messages: [{ id: `m-${id}`, from_me: false, content: { type: "text", text: `hi from ${title}` }, timestamp: new Date().toISOString() }],
    kind: "dm", transport: "direct",
  });

  it("never sends to a conversation other than the one on screen", async () => {
    // Opening Alice, then Bob, with Alice's fetch landing last used to put
    // Alice on screen while the composer sent to Bob (audit UI-02).
    const pending = {};
    const api = readyApi({
      listConversations: [conv, bob],
      getConversation: (id) => new Promise((r) => { pending[id] = r; }),
      sendMessage: vi.fn(async () => {}),
    });
    render(<App />);
    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));
    await userEvent.click(await screen.findByRole("option", { name: /Bob/ }));
    pending["chat-2"](detail("chat-2", "Bob"));
    await screen.findByText("hi from Bob");
    pending["chat-1"](detail("chat-1", "Alice"));
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByText("hi from Alice")).toBeNull();

    const box = document.getElementById("composer-input");
    await userEvent.type(box, "for bob{Enter}");
    await waitFor(() => expect(api.sendMessage).toHaveBeenCalled());
    expect(api.sendMessage.mock.calls.every(([id]) => id === "chat-2")).toBe(true);
  });

  // A send the backend refuses must keep the text: the composer is the only
  // copy of it, and the error is the only sign anything went wrong.
  it("keeps the draft when a send is refused, and says why", async () => {
    readyApi({
      getConversation: detail("chat-1", "Alice"),
      sendMessage: vi.fn(async () => { throw "Not connected — this conversation has no session"; }),
    });
    render(<App />);
    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));
    await screen.findByText("hi from Alice");
    const box = document.getElementById("composer-input");
    await userEvent.type(box, "do not lose me{Enter}");
    await waitFor(() => expect(box.value).toBe("do not lose me"));
    const { subscribe } = await import("./lib/toast.js");
    let shown = [];
    subscribe((l) => { shown = l; })();
    expect(shown.some((t) => t.level === "error" && /Not connected/.test(t.message))).toBe(true);
  });

  it("deleting the open conversation closes it", async () => {
    const api = readyApi({ getConversation: detail("chat-1", "Alice"), deleteChat: vi.fn(async () => {}) });
    render(<App />);
    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));
    await screen.findByText("hi from Alice");
    await userEvent.click(screen.getByRole("button", { name: "More" }));
    await userEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    const dialog = await screen.findByRole("dialog", { name: "Delete conversation" });
    await userEvent.click(within(dialog).getByRole("button", { name: /Delete/ }));
    await waitFor(() => expect(api.deleteChat).toHaveBeenCalledWith("chat-1"));
    await waitFor(() => expect(screen.queryByText("hi from Alice")).toBeNull());
  });
});


// Reads the toast store, which the shell writes to and nothing on screen
// exposes until <Toasts> renders a frame.
async function toasts() {
  const { subscribe } = await import("./lib/toast.js");
  let shown = [];
  subscribe((l) => { shown = l; })();
  return shown;
}

describe("App — getting in", () => {
  it("a wrong password keeps the lock screen up and says so; the right one opens the app", async () => {
    let unlocked = false;
    const api = installApi({
      authStatus: async () => (unlocked ? ready : { ...ready, state: "unlock" }),
      unlock: vi.fn(async (pw) => {
        if (pw !== "correct horse battery") throw "Wrong password";
        unlocked = true;
      }),
      listConversations: [], listTransfers: [], lockState: false, partyList: [],
    });
    render(<App />);
    const field = await screen.findByPlaceholderText("Password");
    await userEvent.type(field, "guess{Enter}");
    expect(await screen.findByText(/Wrong password/)).toBeTruthy();
    expect(screen.queryByRole("navigation")).toBeNull();
    expect(field.value).toBe("");
    await userEvent.type(field, "correct horse battery{Enter}");
    await screen.findByRole("navigation", { name: "Primary" });
    expect(api.unlock).toHaveBeenCalledTimes(2);
  });

  it("a new identity is offered a backup once, before the app opens", async () => {
    let created = false;
    const api = installApi({
      authStatus: async () => (created ? ready : { ...ready, state: "set_password" }),
      setPassword: vi.fn(async () => { created = true; }),
      listConversations: [], listTransfers: [], lockState: false, partyList: [],
    });
    render(<App />);
    const pw = "a perfectly long passphrase";
    await userEvent.type(await screen.findByPlaceholderText(/New password/), pw);
    await userEvent.type(screen.getByPlaceholderText("Confirm password"), pw);
    await userEvent.click(screen.getByRole("button", { name: /Create identity/ }));
    await waitFor(() => expect(api.setPassword).toHaveBeenCalledWith(pw));
    // Not straight into the app: this is the one moment a backup is cheap.
    await screen.findByRole("button", { name: /Save backup file/ });
    expect(screen.queryByRole("navigation")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: /Skip for now/ }));
    await screen.findByRole("navigation", { name: "Primary" });
  });
});

describe("App — shell controls", () => {
  it("the lock button stops new peers, and a refused lock does not claim to be locked", async () => {
    let fail = false;
    const api = readyApi({
      setLocked: vi.fn(async () => { if (fail) throw "listener could not be stopped"; }),
    });
    render(<App />);
    const lock = await screen.findByTitle(/Lock the conversation/);
    await userEvent.click(lock);
    await waitFor(() => expect(api.setLocked).toHaveBeenCalledWith(true));
    const unlock = await screen.findByTitle(/Conversation locked/);
    // A failure must leave the control showing what is true — still locked.
    fail = true;
    await userEvent.click(unlock);
    await waitFor(() => expect(api.setLocked).toHaveBeenLastCalledWith(false));
    expect(screen.getByTitle(/Conversation locked/)).toBeTruthy();
    expect((await toasts()).some((t) => t.level === "error" && /could not be stopped/.test(t.message))).toBe(true);
  });

  it("an unreadable history raises the banner, and starting over moves the file aside", async () => {
    const api = readyApi({
      historyStatus: { path: "/data/history.json.enc", error: "decryption failed" },
      setAsideHistory: vi.fn(async () => "history.json.enc.unreadable-1"),
    });
    render(<App />);
    expect(await screen.findByText(/could not be read/)).toBeTruthy();
    // Two steps: the first click only asks.
    await userEvent.click(screen.getByRole("button", { name: /Start a new history/ }));
    expect(api.setAsideHistory).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /Yes, start new/ }));
    await waitFor(() => expect(api.setAsideHistory).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByText(/could not be read/)).toBeNull());
    expect((await toasts()).some((t) => /unreadable-1/.test(t.message))).toBe(true);
  });

  it("a readable history raises no banner", async () => {
    // Absence is asserted only once the answer has been applied: checking as
    // soon as the shell paints passes before the status has even arrived.
    let answered;
    const applied = new Promise((r) => { answered = r; });
    readyApi({
      historyStatus: async () => {
        queueMicrotask(answered);
        return { path: "/data/history.json.enc", error: null };
      },
    });
    render(<App />);
    await screen.findByRole("navigation", { name: "Primary" });
    await act(() => applied);
    expect(screen.queryByText(/could not be read/)).toBeNull();
  });
});

describe("App — conversation actions", () => {
  const detail = {
    id: "chat-1", title: "Alice", peer_fingerprint: "ab".repeat(32), participants: [],
    created_at: new Date().toISOString(), is_host_placeholder: false,
    messages: [{ id: "m1", from_me: false, content: { type: "text", text: "hello there" }, timestamp: new Date().toISOString() }],
    kind: "dm", transport: "direct",
  };
  async function openAlice(over) {
    const api = readyApi({ getConversation: detail, ...over });
    render(<App />);
    await userEvent.click(await screen.findByRole("option", { name: /Alice/ }));
    await screen.findByText("hello there");
    return api;
  }

  it("Verify opens the trust prompt only when one is pending for this conversation", async () => {
    let pending = null;
    await openAlice({ pendingFingerprint: async () => pending });
    // Nothing pending: the read-only details, never an accept button.
    await userEvent.click(screen.getByRole("button", { name: "Verify fingerprint" }));
    const info = await screen.findByRole("dialog", { name: "Alice" });
    expect(within(info).queryByRole("button", { name: /trust|accept|match/i })).toBeNull();
    await userEvent.click(within(info).getAllByRole("button", { name: /close/i })[0]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // A prompt for another conversation must not be offered here either.
    pending = { chat_id: "chat-9", peer_name: "Mallory", fingerprint: "cd".repeat(32), sas: null };
    await userEvent.click(screen.getByRole("button", { name: "Verify fingerprint" }));
    expect(await screen.findByRole("dialog", { name: "Alice" })).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: /Mallory/ })).toBeNull();
    await userEvent.click(within(screen.getByRole("dialog")).getAllByRole("button", { name: /close/i })[0]);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    pending = { chat_id: "chat-1", peer_name: "Alice", fingerprint: "ab".repeat(32), sas: null };
    await userEvent.click(screen.getByRole("button", { name: "Verify fingerprint" }));
    expect(await screen.findByRole("dialog", { name: "Verify Alice" })).toBeTruthy();
  });

  it("rename starts from the current name and saves the trimmed one", async () => {
    const api = await openAlice({ renameChat: vi.fn(async () => {}) });
    await userEvent.click(screen.getByRole("button", { name: "More" }));
    await userEvent.click(screen.getByRole("button", { name: /Rename/ }));
    const dialog = await screen.findByRole("dialog", { name: "Rename conversation" });
    const field = within(dialog).getByRole("textbox");
    expect(field.value).toBe("Alice");
    await userEvent.clear(field);
    await userEvent.type(field, "  Alice (work)  ");
    await userEvent.click(within(dialog).getByRole("button", { name: /Save/ }));
    await waitFor(() => expect(api.renameChat).toHaveBeenCalledWith("chat-1", "Alice (work)"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("shows only this conversation's live transfers, and acts on the one clicked", async () => {
    const transfer = (id, chat, status, file) => ({
      id, chat_id: chat, status, file_name: file, filename: file, name: file,
      size: 1024, total: 1024, bytes: 0, progress: 0, direction: "incoming",
    });
    const api = await openAlice({
      listTransfers: [
        transfer("t-offer", "chat-1", "awaiting", "offer.pdf"),
        transfer("t-done", "chat-1", "done", "finished.pdf"),
        transfer("t-other", "chat-2", "awaiting", "elsewhere.pdf"),
      ],
      acceptTransfer: vi.fn(async () => {}),
      declineTransfer: vi.fn(async () => { throw "the peer already cancelled"; }),
    });
    // Only one offer belongs on screen: the finished one is already a
    // message, and the other conversation's is not this user's decision here.
    const accept = await screen.findAllByRole("button", { name: "Accept" });
    expect(accept).toHaveLength(1);
    await userEvent.click(accept[0]);
    await waitFor(() => expect(api.acceptTransfer).toHaveBeenCalledWith("t-offer"));
    await userEvent.click(screen.getByRole("button", { name: "Decline" }));
    await waitFor(() => expect(api.declineTransfer).toHaveBeenCalledWith("t-offer"));
    await waitFor(async () =>
      expect((await toasts()).some((t) => t.level === "error" && /already cancelled/.test(t.message))).toBe(true));
  });
});
