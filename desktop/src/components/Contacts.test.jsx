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
});
