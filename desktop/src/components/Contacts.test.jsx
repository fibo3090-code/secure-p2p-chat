// @vitest-environment jsdom
//
// The contacts directory. Two rules here are security-relevant rather than
// cosmetic: an invite link is not verification (so an imported contact says so),
// and deleting a contact throws away a verified fingerprint *and* lifts any
// block — which the confirmation has to admit before the user agrees to it.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { Contacts } from "./Contacts.jsx";
import { stubApi } from "../test/render.jsx";

function installApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

const alice = {
  id: "c1", name: "Alice", fingerprint: "a".repeat(64), address: "192.168.1.21:12345",
  trust: "verified", reachable: true, relay_only: false, blocked: false,
};

beforeEach(() => installApi({ listContacts: [] }));

describe("Contacts", () => {
  it("says there is nothing here rather than showing an empty grid", async () => {
    render(<Contacts />);
    expect(await screen.findByText(/No contacts yet/)).toBeTruthy();
  });

  it("lists a saved contact with its trust state and address", async () => {
    installApi({ listContacts: [alice] });
    render(<Contacts />);
    expect(await screen.findByText("Alice")).toBeTruthy();
    expect(screen.getByText("192.168.1.21:12345")).toBeTruthy();
    // The badge is icon-only in `mini` form, so assert the state it encodes
    // rather than a label that is deliberately not drawn.
    expect(document.querySelector(".trust-badge.trust-ok")).toBeTruthy();
    expect(screen.getByText("a".repeat(64))).toBeTruthy();
  });

  it("imports an invite link and reloads the list", async () => {
    const importInvite = vi.fn(async () => ({ contact: { ...alice, name: "Imported" }, signed: true }));
    const api = installApi({ importInvite, listContacts: async () => [] });
    render(<Contacts />);

    await userEvent.type(screen.getByPlaceholderText(/Paste an invite link/), "chat-p2p://invite/abc");
    await userEvent.click(screen.getByRole("button", { name: "Add" }));

    expect(importInvite).toHaveBeenCalledWith("chat-p2p://invite/abc");
    // The list is re-fetched rather than patched locally, so what is on screen
    // is what the backend actually stored.
    expect(api.listContacts.mock.calls.length).toBeGreaterThan(1);
  });

  it("shows the import failure instead of failing silently", async () => {
    installApi({
      listContacts: [],
      importInvite: async () => { throw new Error("invite fingerprint does not match its key"); },
    });
    render(<Contacts />);
    await userEvent.type(screen.getByPlaceholderText(/Paste an invite link/), "chat-p2p://invite/bad");
    await userEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByText(/does not match its key/)).toBeTruthy();
  });

  it("blocks and unblocks through the bridge", async () => {
    const api = installApi({ listContacts: [alice] });
    render(<Contacts />);
    await screen.findByText("Alice");
    await userEvent.click(screen.getByRole("button", { name: "Block" }));
    expect(api.blockContact).toHaveBeenCalledWith("c1");

    installApi({ listContacts: [{ ...alice, trust: "blocked", blocked: true }] });
    render(<Contacts />);
    const unblock = await screen.findAllByRole("button", { name: "Unblock" });
    await userEvent.click(unblock[0]);
    expect(bridge.api.unblockContact).toHaveBeenCalledWith("c1");
  });

  it("will not offer Connect to a contact with no way to reach them", async () => {
    installApi({ listContacts: [{ ...alice, reachable: false }] });
    render(<Contacts />);
    await screen.findByText("Alice");
    expect(screen.getByRole("button", { name: "Connect" }).disabled).toBe(true);
    expect(screen.getByText(/no way to reach them/)).toBeTruthy();
  });

  it("confirms a delete, and admits it discards the verified fingerprint", async () => {
    const api = installApi({ listContacts: [alice] });
    render(<Contacts />);
    await screen.findByText("Alice");
    await userEvent.click(screen.getByRole("button", { name: "Delete contact" }));

    const dialog = screen.getByRole("dialog", { name: "Delete contact" });
    expect(within(dialog).getByText(/compare the safety code with them again/)).toBeTruthy();
    expect(api.removeContact).not.toHaveBeenCalled();

    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(api.removeContact).toHaveBeenCalledWith("c1");
  });

  it("warns that deleting a blocked contact lets them back in", async () => {
    installApi({ listContacts: [{ ...alice, trust: "blocked", blocked: true }] });
    render(<Contacts />);
    await screen.findByText("Alice");
    await userEvent.click(screen.getByRole("button", { name: "Delete contact" }));
    // The block lives only on the contact, so deleting it silently unblocks.
    expect(screen.getByText(/lets them connect to you again/)).toBeTruthy();
  });

  it("can be cancelled without deleting anything", async () => {
    const api = installApi({ listContacts: [alice] });
    render(<Contacts />);
    await screen.findByText("Alice");
    await userEvent.click(screen.getByRole("button", { name: "Delete contact" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.removeContact).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  // An unsigned (v1) link carries no proof of who made it. Importing one with
  // the same quiet success as a signed one told the user nothing.
  it("warns when an invite link is unsigned, and not when it is signed", async () => {
    const { subscribe, dismiss } = await import("../lib/toast.js");
    const shown = () => { let l = []; subscribe((x) => { l = x; })(); return l; };
    shown().forEach((t) => dismiss(t.id));
    const importInvite = vi.fn(async (link) =>
      ({ contact: { ...alice, name: "Bob" }, signed: !link.endsWith("v1") }));
    installApi({ importInvite, listContacts: [] });
    render(<Contacts />);
    const box = screen.getByPlaceholderText(/Paste an invite link/);
    const add = screen.getByRole("button", { name: "Add" });

    await userEvent.type(box, "chat-p2p://invite/v1");
    await userEvent.click(add);
    await vi.waitFor(() => expect(shown().some((t) => /unsigned invite link/.test(t.message) && t.level === "error")).toBe(true));

    shown().forEach((t) => dismiss(t.id));
    await userEvent.type(box, "chat-p2p://invite/v2");
    await userEvent.click(add);
    await vi.waitFor(() => expect(shown().some((t) => t.message === "Added Bob.")).toBe(true));
    expect(shown().some((t) => /unsigned/.test(t.message))).toBe(false);
  });

  it("connects to a contact, and shows why when it cannot", async () => {
    const onConnected = vi.fn();
    const connectContact = vi.fn(async (id) => { if (id === "c2") throw "This contact is blocked"; });
    installApi({
      listContacts: [alice, { ...alice, id: "c2", name: "Mallory", fingerprint: "b".repeat(64) }],
      connectContact,
    });
    render(<Contacts onConnected={onConnected} />);
    const card = (name) => screen.getByText(name).closest(".contact-card");
    await screen.findByText("Alice");
    await userEvent.click(within(card("Alice")).getByRole("button", { name: "Connect" }));
    expect(connectContact).toHaveBeenCalledWith("c1");
    await vi.waitFor(() => expect(onConnected).toHaveBeenCalledTimes(1));

    await userEvent.click(within(card("Mallory")).getByRole("button", { name: "Connect" }));
    expect(await screen.findByText(/This contact is blocked/)).toBeTruthy();
    expect(onConnected).toHaveBeenCalledTimes(1);
  });

  // A refused clipboard write must say so: silently doing nothing invites the
  // user to paste a stale fingerprint into a verification conversation.
  it("says whether the fingerprint was actually copied", async () => {
    const { subscribe, dismiss } = await import("../lib/toast.js");
    const shown = () => { let l = []; subscribe((x) => { l = x; })(); return l; };
    shown().forEach((t) => dismiss(t.id));
    installApi({ listContacts: [alice] });
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<Contacts />);
    await screen.findByText("Alice");
    const copy = screen.getByRole("button", { name: /copy fingerprint/i });
    await userEvent.click(copy);
    expect(writeText).toHaveBeenCalledWith("a".repeat(64));
    await vi.waitFor(() => expect(shown().some((t) => t.message === "Fingerprint copied")).toBe(true));

    writeText.mockRejectedValueOnce(new Error("denied"));
    await userEvent.click(copy);
    await vi.waitFor(() => expect(shown().some((t) => /Could not copy/.test(t.message))).toBe(true));
  });
});

