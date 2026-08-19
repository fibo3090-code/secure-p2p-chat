// @vitest-environment jsdom
//
// Toasts are the app's only channel for "not delivered", "transfer interrupted"
// and "verification failed". Two rules follow from that and are pinned here:
// errors go in an assertive live region (a screen-reader user must be told), and
// errors do not expire on their own (four seconds is not enough to read one).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Toasts } from "./Toasts.jsx";
import { toast, dismiss, subscribe, DEFAULT_TTL_MS, MAX_TOASTS } from "../lib/toast.js";

// The store is module-level and shared, so each test starts from empty.
function clearAll() {
  let current = [];
  const unsub = subscribe((l) => { current = l; });
  unsub();
  current.forEach((t) => dismiss(t.id));
}

beforeEach(clearAll);
afterEach(() => { vi.useRealTimers(); clearAll(); });

describe("Toasts", () => {
  it("puts errors in an assertive region and everything else in a polite one", () => {
    render(<Toasts />);
    act(() => { toast("could not send", "error"); toast("copied", "success"); });

    const alert = screen.getByRole("alert");
    const status = screen.getByRole("status");
    expect(alert.getAttribute("aria-live")).toBe("assertive");
    expect(status.getAttribute("aria-live")).toBe("polite");
    expect(alert.textContent).toContain("could not send");
    expect(status.textContent).toContain("copied");
  });

  it("gives each toast an accessible name that says it can be dismissed", () => {
    render(<Toasts />);
    act(() => { toast("could not send", "error"); });
    expect(screen.getByRole("button", { name: /^Error: could not send\. Activate to dismiss\.$/ })).toBeTruthy();
  });

  it("dismisses on click", async () => {
    render(<Toasts />);
    act(() => { toast("could not send", "error"); });
    await userEvent.click(screen.getByRole("button", { name: /could not send/ }));
    expect(screen.queryByText("could not send")).toBeNull();
  });

  it("expires a success toast but keeps an error until it is dismissed", () => {
    vi.useFakeTimers();
    render(<Toasts />);
    act(() => { toast("copied", "success"); toast("could not send", "error"); });

    act(() => { vi.advanceTimersByTime(DEFAULT_TTL_MS + 100); });
    expect(screen.queryByText("copied")).toBeNull();
    // An error is the only report the user gets that something failed.
    expect(screen.getByText("could not send")).toBeTruthy();
  });

  it("caps how many are held at once so a flapping connection cannot bury the screen", () => {
    render(<Toasts />);
    act(() => {
      for (let i = 0; i < MAX_TOASTS + 4; i++) toast(`failure ${i}`, "error");
    });
    expect(screen.getAllByRole("button")).toHaveLength(MAX_TOASTS);
    // Oldest go first.
    expect(screen.queryByText("failure 0")).toBeNull();
    expect(screen.getByText(`failure ${MAX_TOASTS + 3}`)).toBeTruthy();
  });

  it("renders both regions even with nothing to show, so they are not created mid-announcement", () => {
    render(<Toasts />);
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByRole("status")).toBeTruthy();
  });
});
