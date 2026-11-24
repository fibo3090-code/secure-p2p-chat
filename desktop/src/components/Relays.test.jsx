// @vitest-environment jsdom
//
// The relay pane. It explains relays and points pairing at New connection; it is
// deliberately honest that saved relays and live routes are not tracked, so one
// of these tests pins that rather than letting a future change fabricate data.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { Relays } from "./Relays.jsx";
import { subscribe, dismiss } from "../lib/toast.js";

// Read the toast store without mounting <Toasts/>.
function toasts() {
  let current = [];
  subscribe((l) => { current = l; })();
  return current;
}
function clearToasts() { toasts().forEach((t) => dismiss(t.id)); }

beforeEach(() => { clearToasts(); });
afterEach(clearToasts);

describe("Relays", () => {
  it("says what a relay is and is not", () => {
    render(<Relays />);
    expect(screen.getByText(/forwards bytes only/)).toBeTruthy();
    expect(screen.getByText(/end-to-end encrypted and fingerprint-verified/)).toBeTruthy();
  });

  it("sends pairing to New connection, in relay mode, instead of a second form", async () => {
    const onPair = vi.fn();
    render(<Relays onPair={onPair} />);
    await userEvent.click(screen.getByRole("button", { name: /Connect with a token/ }));
    expect(onPair).toHaveBeenCalledWith("connect");
    await userEvent.click(screen.getByRole("button", { name: /Open a relay session/ }));
    expect(onPair).toHaveBeenCalledWith("host");
    // No duplicate pairing form on this page.
    expect(screen.queryByPlaceholderText("connection token")).toBeNull();
  });

  it("does not invent saved relays it does not track", () => {
    // Saved relays and live routes are not tracked, so the page must not show
    // a list (or a placeholder apologising for one) as if they were.
    render(<Relays />);
    expect(screen.queryByText(/Saved relays/i)).toBeNull();
  });

  it("offers a runnable command for a self-hosted relay", () => {
    render(<Relays />);
    // A wrong crate name here is a user typing it into a terminal and being told
    // the package does not exist.
    expect(screen.getByText(/-p p2pem-classic -- --relay-server/)).toBeTruthy();
  });

  it("says so when the clipboard refuses, instead of claiming it copied", async () => {
    const writeText = vi.fn(async () => { throw new Error("blocked"); });
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<Relays />);
    await userEvent.click(screen.getAllByRole("button", { name: "Copy" })[0]);
    expect(toasts().at(-1).message).toMatch(/Could not copy/);
  });
});
