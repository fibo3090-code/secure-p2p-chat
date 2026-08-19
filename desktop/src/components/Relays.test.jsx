// @vitest-environment jsdom
//
// The relay pane. It is deliberately honest about what the backend does and does
// not do — saved relays and live routes are not tracked yet — so one of these
// tests pins the empty state rather than letting a future change quietly
// fabricate data there.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { Relays } from "./Relays.jsx";
import { stubApi } from "../test/render.jsx";
import { subscribe, dismiss } from "../lib/toast.js";

function useApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

// Read the toast store without mounting <Toasts/>.
function toasts() {
  let current = [];
  subscribe((l) => { current = l; })();
  return current;
}
function clearToasts() { toasts().forEach((t) => dismiss(t.id)); }

beforeEach(() => { clearToasts(); useApi({}); });
afterEach(clearToasts);

describe("Relays", () => {
  it("says what a relay is and is not", () => {
    render(<Relays />);
    expect(screen.getByText(/forwards bytes only/)).toBeTruthy();
    expect(screen.getByText(/end-to-end encrypted and fingerprint-verified/)).toBeTruthy();
  });

  it("refuses to pair without an address, and says which field is missing", async () => {
    const api = useApi({});
    render(<Relays />);
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(api.connectViaRelay).not.toHaveBeenCalled();
    expect(toasts().at(-1).message).toMatch(/Enter the relay address/);

    await userEvent.type(screen.getByPlaceholderText("relay.example.com:9000"), "relay.test:9000");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(api.connectViaRelay).not.toHaveBeenCalled();
    expect(toasts().at(-1).message).toMatch(/connection token/);
  });

  it("pairs with the trimmed address and token", async () => {
    const api = useApi({});
    const onConnected = vi.fn();
    render(<Relays onConnected={onConnected} />);
    await userEvent.type(screen.getByPlaceholderText("relay.example.com:9000"), "  relay.test:9000  ");
    await userEvent.type(screen.getByPlaceholderText("connection token"), " rly_abc ");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));

    expect(api.connectViaRelay).toHaveBeenCalledWith("relay.test:9000", "rly_abc");
    expect(onConnected).toHaveBeenCalled();
  });

  it("opens a relay session and shows the token to share", async () => {
    useApi({ hostViaRelay: async () => "rly_deadbeef" });
    render(<Relays />);
    await userEvent.type(screen.getByPlaceholderText("relay.example.com:9000"), "relay.test:9000");
    await userEvent.click(screen.getByRole("button", { name: "Host" }));
    expect(await screen.findByText("rly_deadbeef")).toBeTruthy();
  });

  it("reports a failed pairing instead of silently doing nothing", async () => {
    useApi({ connectViaRelay: async () => { throw new Error("relay refused the token"); } });
    render(<Relays />);
    await userEvent.type(screen.getByPlaceholderText("relay.example.com:9000"), "relay.test:9000");
    await userEvent.type(screen.getByPlaceholderText("connection token"), "rly_abc");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(toasts().at(-1).message).toMatch(/relay refused the token/);
    expect(toasts().at(-1).level).toBe("error");
  });

  it("shows an explicit empty state rather than inventing saved relays", () => {
    render(<Relays />);
    expect(screen.getByText(/aren't tracked yet/)).toBeTruthy();
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
