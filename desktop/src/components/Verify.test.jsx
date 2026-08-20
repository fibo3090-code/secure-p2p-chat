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

import { Verify } from "./Verify.jsx";
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
    expect(screen.getByText("418 902 🎃🎈🎁")).toBeTruthy();
    expect(screen.getByText(/Read this aloud with Alice/)).toBeTruthy();
    // The 64-char compare is the backstop, not the instruction.
    expect(screen.getByText(/Full fingerprint \(advanced\)/)).toBeTruthy();
  });

  it("falls back to the fingerprint when the peer sent no SAS", () => {
    render(<Verify req={{ ...req, sas: null }} onClose={() => {}} />);
    expect(screen.getByText(/Compare this grid/)).toBeTruthy();
    expect(screen.queryByText(/Read this aloud/)).toBeNull();
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
});
