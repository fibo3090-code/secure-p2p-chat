// @vitest-environment jsdom
//
// The TOFU prompt. This is the one dialog in the app where getting the UI wrong
// is a security failure rather than an annoyance: it must lead with the SAS,
// must not be dismissable while a session waits on the answer, and must not
// close on a *failed* confirmation — the Rust side only clears the pending
// prompt when the command succeeds, so closing early leaves a trusted-looking
// conversation that was never actually trusted.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { Verify, warningText } from "./Verify.jsx";
import { stubApi } from "../test/render.jsx";

function installApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

const req = {
  chat_id: "chat-1",
  peer_name: "Alice Smith",
  fingerprint: "ab".repeat(32),
  sas: "418 902 🎃🎈🎁",
};

beforeEach(() => installApi({}));

describe("Verify", () => {
  it("renders nothing when there is no pending request", () => {
    render(<Verify req={null} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("leads with the safety code and demotes the fingerprint to advanced", () => {
    render(<Verify req={req} onClose={() => {}} />);
    // Digits and emoji are laid out apart for reading aloud; both are shown,
    // and a screen reader gets the digits one at a time.
    expect(screen.getByText("418 902")).toBeTruthy();
    expect(screen.getByText("🎃🎈🎁")).toBeTruthy();
    expect(screen.getByRole("img", { name: /Safety code: 4 1 8 9 0 2, then 🎃🎈🎁/ })).toBeTruthy();
    expect(screen.getByText(/Read this aloud with Alice/)).toBeTruthy();
    // The 64-char compare is the backstop, not the instruction.
    expect(screen.getByText(/Full fingerprint \(advanced\)/)).toBeTruthy();
  });

  it("falls back to the fingerprint when the peer sent no SAS", () => {
    render(<Verify req={{ ...req, sas: null }} onClose={() => {}} />);
    expect(screen.getByText(/Compare this grid/)).toBeTruthy();
    expect(screen.queryByText(/Read this aloud/)).toBeNull();
  });

  // CRYPTO-04: a key that is not the one in the dialled contact's invite must
  // not look like a first contact.
  it("leads with a warning when the key is not the one in the contact's invite", () => {
    render(<Verify req={{ ...req, warning: { kind: "invite_mismatch", contact: "Alice", expected: "cd".repeat(32) } }}
      onClose={() => {}} />);
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toMatch(/not the key in Alice's invite/);
    expect(alert.textContent).toContain("cd".repeat(8));
    expect(screen.getByText(/Unexpected key/)).toBeTruthy();
    // The SAS is still the check, and accepting is still possible — but it is
    // worded as trusting a *new* key, not as a routine confirmation.
    expect(screen.getByText("418 902")).toBeTruthy();
    expect(screen.getByRole("button", { name: /trust this new key/ })).toBeTruthy();
  });

  it("warns when a known conversation's key changed", () => {
    render(<Verify req={{ ...req, warning: { kind: "key_changed", previous: "ef".repeat(32) } }} onClose={() => {}} />);
    expect(screen.getByRole("alert").textContent).toMatch(/key has changed/);
  });

  it("shows no warning for an ordinary first use", () => {
    render(<Verify req={{ ...req, warning: null }} onClose={() => {}} />);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText(/Check it's really Alice/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Codes match — trust" })).toBeTruthy();
  });

  it("still warns for a warning kind it does not know", () => {
    expect(warningText({ kind: "something_new" })).toBeTruthy();
    expect(warningText(null)).toBeNull();
  });

  it("cannot be dismissed — a live session is waiting on the answer", async () => {
    const onClose = vi.fn();
    const { container } = render(<Verify req={req} onClose={onClose} />);
    await userEvent.keyboard("{Escape}");
    await userEvent.click(container.querySelector(".modal-scrim"));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("confirms trust with accept=true and closes", async () => {
    const api = installApi({});
    const onClose = vi.fn();
    render(<Verify req={req} onClose={onClose} />);
    await userEvent.click(screen.getByRole("button", { name: /Codes match/ }));
    expect(api.confirmFingerprint).toHaveBeenCalledWith("chat-1", true);
    expect(onClose).toHaveBeenCalled();
  });

  it("rejects with accept=false", async () => {
    const api = installApi({});
    render(<Verify req={req} onClose={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: "Reject" }));
    expect(api.confirmFingerprint).toHaveBeenCalledWith("chat-1", false);
  });

  it("stays open on a failed confirmation, and only then offers a way out", async () => {
    installApi({ confirmFingerprint: async () => { throw new Error("session is gone"); } });
    const onClose = vi.fn();
    render(<Verify req={req} onClose={onClose} />);

    await userEvent.click(screen.getByRole("button", { name: /Codes match/ }));
    expect(await screen.findByText(/session is gone/i)).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();

    // The escape hatch appears only after a failure: a prompt whose session has
    // died can never succeed, and the user must not be trapped in it. Closing
    // this way trusts nothing.
    await userEvent.click(screen.getByRole("button", { name: /close without trusting/i }));
    expect(onClose).toHaveBeenCalled();
  });

  it("clears a previous request's error when a new peer arrives", async () => {
    installApi({ confirmFingerprint: async () => { throw new Error("session is gone"); } });
    const { rerender } = render(<Verify req={req} onClose={() => {}} />);
    await userEvent.click(screen.getByRole("button", { name: /Codes match/ }));
    await screen.findByText(/session is gone/i);

    rerender(<Verify req={{ ...req, chat_id: "chat-2", peer_name: "Bob" }} onClose={() => {}} />);
    // A security dialog must not show the previous request's failure above a
    // fresh decision.
    expect(screen.queryByText(/session is gone/i)).toBeNull();
  });

  it("says the safety code changed when a verified conversation sees a new key", async () => {
    // The real bridge serialises the Rust enum as "Relay", not "relay".
    installApi({ getConversation: { id: "chat-1", peer_fingerprint: "cd".repeat(32), transport: "Relay" } });
    render(<Verify req={req} onClose={() => {}} />);
    // A changed key must not read as a first connection.
    expect(await screen.findByText("Alice's safety code changed")).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toMatch(/different key/);
    // Trusting a changed key is deliberate: Reject has focus, trust is not the hero.
    expect(document.activeElement.textContent).toMatch(/Reject/);
    expect(screen.getByRole("button", { name: /Codes match/ }).className).not.toMatch(/btn-primary/);
  });

  it("stays a plain check when the stored key matches or there is none", async () => {
    installApi({ getConversation: { id: "chat-1", peer_fingerprint: "AB".repeat(32), transport: "Direct" } });
    render(<Verify req={req} onClose={() => {}} />);
    expect(await screen.findByText("Direct connection")).toBeTruthy();
    // Same key in a different case is the same key — no false alarm.
    expect(screen.queryByText(/safety code changed/)).toBeNull();
  });

  // Review of #207/#210: a lookup that failed or timed out left the dialog
  // reading "first use". On the listener side it always failed, so an
  // incoming peer whose key had changed got the calm first-contact dialog.
  // Unknown history is not first use.
  it.each([
    ["fails", () => Promise.reject(new Error("No such conversation"))],
    ["finds nothing", async () => null],
    ["never answers", () => new Promise(() => {})],
  ])("does not call it a first connection when the lookup %s", async (_how, lookup) => {
    installApi({ getConversation: lookup });
    render(<Verify req={req} onClose={() => {}} />);
    expect(await screen.findByText(/couldn't check whether you have talked to Alice before/i,
      {}, { timeout: 2000 })).toBeTruthy();
    expect(screen.getByText("Couldn't check this conversation's history")).toBeTruthy();
    expect(screen.queryByText(/Check it's really Alice/)).toBeNull();
    // Deliberate, like any other alarm: trust is not the primary action.
    expect(screen.getByRole("button", { name: "Codes match — trust this new key" })).toBeTruthy();
  });

  it("names the contact an accept would repoint, and the listener-side mismatch", () => {
    const changed = warningText({ kind: "key_changed", previous: "ef".repeat(32), repoints: "Bob" });
    expect(changed).toMatch(/replaces the key saved for Bob/);
    expect(warningText({ kind: "key_changed", previous: "ef".repeat(32), repoints: null }))
      .not.toMatch(/replaces/);
    const fromAddress = warningText({ kind: "contact_address_mismatch", contact: "Carol", expected: "cd".repeat(32) });
    expect(fromAddress).toMatch(/comes from Carol's address, but it is not their key \(expected cdcdcdcdcdcdcdcd…\)/);
    expect(fromAddress).toMatch(/does not change the key saved for Carol/);
  });

  it("holds the trust button until the lookup has settled", async () => {
    let resolve;
    installApi({ getConversation: () => new Promise((r) => { resolve = r; }) });
    render(<Verify req={req} onClose={() => {}} />);
    expect(screen.getByRole("button", { name: /Codes match/ }).disabled).toBe(true);
    resolve({ id: "chat-1", peer_fingerprint: null, transport: "Direct" });
    await screen.findByText("Direct connection");
    expect(screen.getByRole("button", { name: /Codes match/ }).disabled).toBe(false);
  });
});
