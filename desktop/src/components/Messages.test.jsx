// @vitest-environment jsdom
//
// The conversation list and the chat pane — the two surfaces where a UI bug
// costs the user a message. Three rules are load-bearing enough to be pinned
// here: a composer with no session must refuse the keystroke rather than eat a
// paragraph, a second concurrent file send must be impossible, and opening a
// peer's executable must become a question rather than an execution.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// The components import `{ api }` from the bridge, so that one object is
// swapped for a stub. `real` keeps a handle on the genuine command set, which
// `stubApi` uses to reject a test that stubs a command that no longer exists.
const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { ConvList, ChatPane, rowStatusLabel } from "./Messages.jsx";
import { stubApi, contactFixture, convFixture } from "../test/render.jsx";

function useApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

beforeEach(() => {
  useApi({});
});

describe("rowStatusLabel", () => {
  it("puts the unread count first — it is what decides whether you open the row", () => {
    expect(rowStatusLabel(convFixture({ unread: 3 }))).toMatch(/^3 unread messages/);
    expect(rowStatusLabel(convFixture({ unread: 1 }))).toMatch(/^1 unread message,/);
    expect(rowStatusLabel(convFixture({ unread: 0 }))).not.toMatch(/unread/);
  });

  it("says out loud everything the row only draws as an icon", () => {
    const label = rowStatusLabel(
      convFixture({ trust: "unverified", transport: "relay", state: "hosting", kind: "group" }),
    );
    expect(label).toContain("not verified");
    expect(label).toContain("over a relay");
    expect(label).toContain("hosting");
    expect(label).toContain("group");
  });
});

describe("ConvList", () => {
  const rows = [
    convFixture({ id: "a", name: "Alice", unread: 2 }),
    convFixture({ id: "b", name: "Bob", state: "offline", trust: "unverified" }),
  ];

  it("is a listbox of options, with the active row marked selected", () => {
    render(<ConvList contacts={rows} activeId="b" onSelect={() => {}} onAdd={() => {}} query="" setQuery={() => {}} />);
    const list = screen.getByRole("listbox", { name: "Conversations" });
    const options = within(list).getAllByRole("option");
    expect(options).toHaveLength(2);
    expect(options[1].getAttribute("aria-selected")).toBe("true");
    expect(options[0].getAttribute("aria-selected")).toBe("false");
  });

  it("announces the unread total in a live region", () => {
    render(<ConvList contacts={rows} activeId={null} onSelect={() => {}} onAdd={() => {}} query="" setQuery={() => {}} />);
    expect(screen.getByRole("status").textContent).toBe("2 unread messages");
  });

  it("names each row with its unread count and trust state", () => {
    render(<ConvList contacts={rows} activeId={null} onSelect={() => {}} onAdd={() => {}} query="" setQuery={() => {}} />);
    expect(screen.getByRole("option", { name: /Alice\. 2 unread messages/ })).toBeTruthy();
    expect(screen.getByRole("option", { name: /Bob\..*not verified/ })).toBeTruthy();
  });

  it("filters on the query and selects by id", async () => {
    const onSelect = vi.fn();
    render(<ConvList contacts={rows} activeId={null} onSelect={onSelect} onAdd={() => {}} query="ali" setQuery={() => {}} />);
    const options = screen.getAllByRole("option");
    expect(options).toHaveLength(1);
    await userEvent.click(options[0]);
    expect(onSelect).toHaveBeenCalledWith("a");
  });

  it("offers a way out when there is nothing to select", async () => {
    const onAdd = vi.fn();
    render(<ConvList contacts={[]} activeId={null} onSelect={() => {}} onAdd={onAdd} query="" setQuery={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /Start a connection/ }));
    expect(onAdd).toHaveBeenCalled();
  });
});

describe("ChatPane composer", () => {
  const base = {
    draft: "",
    setDraft: () => {},
    onSend: () => {},
    transfers: [],
    onStart: () => {},
  };

  it("disables the composer and explains why when there is no session", () => {
    render(<ChatPane {...base} contact={contactFixture({ state: "offline", messages: [] })} />);
    const box = screen.getByRole("textbox");
    // A send that cannot be delivered must never look like it worked: the
    // backend errors, and the composer refuses the keystroke in the first place.
    expect(box.disabled).toBe(true);
    expect(screen.getByRole("status").textContent).toMatch(/Not connected/);
  });

  it("sends on Enter when connected, and not on Shift+Enter", async () => {
    const onSend = vi.fn();
    render(<ChatPane {...base} draft="hi" onSend={onSend} contact={contactFixture()} />);
    const box = screen.getByRole("textbox");
    await userEvent.click(box);
    await userEvent.keyboard("{Shift>}{Enter}{/Shift}");
    expect(onSend).not.toHaveBeenCalled();
    await userEvent.keyboard("{Enter}");
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("refuses a second concurrent file send", () => {
    // `FileChunk` carries no transfer id, so two concurrent sends on one
    // conversation interleave on the wire and corrupt both files. The backend
    // refuses the second; the button must not pretend otherwise.
    const { rerender } = render(<ChatPane {...base} contact={contactFixture()} />);
    const clip = () => document.querySelector(".composer-clip");
    expect(clip().disabled).toBe(false);

    rerender(
      <ChatPane {...base} contact={contactFixture()}
        transfers={[{ id: "t1", direction: "outgoing", status: "active", filename: "big.iso", size: 10, received: 1, cancellable: true }]} />,
    );
    expect(clip().disabled).toBe(true);
    expect(clip().getAttribute("title")).toMatch(/Already sending/);
  });

  it("carries the skip-link target on the message box", () => {
    render(<ChatPane {...base} contact={contactFixture()} />);
    expect(screen.getByRole("textbox").id).toBe("composer-input");
  });
});

describe("ChatPane transfers", () => {
  const base = { draft: "", setDraft: () => {}, onSend: () => {}, onStart: () => {} };

  it("offers accept/decline for an incoming offer instead of a progress bar", async () => {
    const onAccept = vi.fn();
    const onDecline = vi.fn();
    const t = { id: "t9", direction: "incoming", status: "awaiting", filename: "notes.txt", size: 100, received: 0 };
    render(
      <ChatPane {...base} contact={contactFixture()} transfers={[t]}
        onAcceptTransfer={onAccept} onDeclineTransfer={onDecline} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Accept" }));
    expect(onAccept).toHaveBeenCalledWith(t);
    await userEvent.click(screen.getByRole("button", { name: "Decline" }));
    expect(onDecline).toHaveBeenCalledWith(t);
  });

  it("surfaces a failure with its reason rather than a bare percentage", () => {
    render(
      <ChatPane {...base} contact={contactFixture()}
        transfers={[{ id: "t1", direction: "outgoing", status: "failed", filename: "x", size: 10, received: 2, error: "peer disconnected" }]} />,
    );
    expect(screen.getByText("peer disconnected")).toBeTruthy();
  });

  it("cancels through the bridge when asked", async () => {
    const onCancel = vi.fn();
    render(
      <ChatPane {...base} contact={contactFixture()} onCancelTransfer={onCancel}
        transfers={[{ id: "t1", direction: "outgoing", status: "active", filename: "x", size: 10, received: 2, cancellable: true }]} />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Cancel transfer" }));
    expect(onCancel).toHaveBeenCalledWith("t1");
  });
});

describe("ChatPane file cards", () => {
  const base = { draft: "", setDraft: () => {}, onSend: () => {}, transfers: [], onStart: () => {} };
  const fileMsg = (over = {}) => ({
    kind: "file", id: "f1", ts: Date.now(), from: "them", name: "holiday.jpg.exe",
    size: "8 MB", progress: 100, t: "12:00", hasPath: true, path: "/dl/holiday.jpg.exe", dir: "/dl",
    ...over,
  });

  it("turns a blocked open into a question, and only runs it after confirmation", async () => {
    const openFile = vi.fn(async (_id, _msg, _reveal, confirm) =>
      confirm ? { opened: true, blocked: null, filename: "holiday.jpg.exe" }
              : { opened: false, blocked: "exe", filename: "holiday.jpg.exe" });
    useApi({ openFile });

    render(<ChatPane {...base} contact={contactFixture({ messages: [fileMsg()] })} />);
    await userEvent.click(screen.getByTitle("Open file"));

    // The dialog is the whole point: opening a peer's .exe is running their code.
    expect(await screen.findByRole("dialog", { name: /run as a program/i })).toBeTruthy();
    expect(openFile).toHaveBeenLastCalledWith("chat-1", "f1", false);

    await userEvent.click(screen.getByRole("button", { name: /Run it anyway/ }));
    expect(openFile).toHaveBeenLastCalledWith("chat-1", "f1", false, true);
  });

  it("lets the user back out of a blocked open without running anything", async () => {
    const openFile = vi.fn(async () => ({ opened: false, blocked: "exe", filename: "x.exe" }));
    useApi({ openFile });

    render(<ChatPane {...base} contact={contactFixture({ messages: [fileMsg()] })} />);
    await userEvent.click(screen.getByTitle("Open file"));
    await screen.findByRole("dialog");
    await userEvent.click(screen.getByRole("button", { name: /Don't open/ }));

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(openFile).toHaveBeenCalledTimes(1);
  });

  it("never gates revealing in the file manager — it launches nothing", async () => {
    const openFile = vi.fn(async () => ({ opened: true, blocked: null, filename: null }));
    useApi({ openFile });

    render(<ChatPane {...base} contact={contactFixture({ messages: [fileMsg()] })} />);
    await userEvent.click(screen.getByTitle("Show in folder"));
    expect(openFile).toHaveBeenCalledWith("chat-1", "f1", true);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says where a received file actually landed", () => {
    render(<ChatPane {...base} contact={contactFixture({ messages: [fileMsg({ dir: "/home/me/Elsewhere" })] })} />);
    // Hardcoding "Downloads" was wrong for anyone who picked another folder, and
    // this card is the only place the app ever says.
    expect(document.body.textContent).toContain("saved to /home/me/Elsewhere");
  });
});

describe("ChatPane empty states", () => {
  const base = { draft: "", setDraft: () => {}, onSend: () => {}, transfers: [] };

  it("explains the three ways to connect on a first run", async () => {
    const onStart = vi.fn();
    render(<ChatPane {...base} contact={null} isFirstRun onStart={onStart} />);
    expect(screen.getByText(/get you connected/i)).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Send them an invite link/ }));
    expect(onStart).toHaveBeenCalledWith("invite");
  });

  it("falls back to a plain prompt when there are conversations to pick", () => {
    render(<ChatPane {...base} contact={null} isFirstRun={false} onStart={() => {}} />);
    expect(screen.getByText("Select a conversation")).toBeTruthy();
  });
});
