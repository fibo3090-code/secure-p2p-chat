// @vitest-environment jsdom
//
// Settings. Two behaviours here are more than cosmetic: a toggle the backend
// rejects has to roll *back* (leaving it flipped tells the user a setting is on
// that is not), and the identity-backup row has to state plainly whether a
// backup exists — losing the identity file is the one failure this app cannot
// undo, and an Export button with no indication either way says nothing.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { Settings } from "./Settings.jsx";
import { stubApi } from "../test/render.jsx";
import { subscribe, dismiss } from "../lib/toast.js";

function useApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}
function toasts() {
  let current = [];
  subscribe((l) => { current = l; })();
  return current;
}
function clearToasts() { toasts().forEach((t) => dismiss(t.id)); }

const settings = {
  download_dir: "/home/me/Downloads",
  enable_notifications: true,
  enable_typing_indicators: true,
  auto_host_on_startup: false,
  listen_port: 12345,
  enable_upnp: false,
  auto_accept_files: false,
  auto_connect: false,
  enable_mdns: false,
  identity_backed_up_at: null,
};

const identity = {
  state: "ready", name: "Maya", fingerprint: "ab".repeat(32), min_password_len: 12,
};

beforeEach(() => { clearToasts(); useApi({ getSettings: settings }); });
afterEach(clearToasts);

describe("Settings", () => {
  it("shows the identity with its grouped fingerprint", async () => {
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    expect(await screen.findByLabelText("Display name")).toBeTruthy();
    expect(screen.getByLabelText("Display name").value).toBe("Maya");
    expect(screen.getByText(/^abab abab/)).toBeTruthy();
  });

  it("warns when the identity has never been backed up", async () => {
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    expect(await screen.findByText("Never backed up")).toBeTruthy();
    expect(screen.getByText(/there is no reset/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /Export now/ })).toBeTruthy();
  });

  it("says when it was last backed up once one exists", async () => {
    useApi({ getSettings: { ...settings, identity_backed_up_at: 1700000000000 } });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    expect(await screen.findByText("Backed up")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Export again/ })).toBeTruthy();
  });

  it("persists a toggle through the bridge", async () => {
    const api = useApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const toggle = await screen.findByText("Desktop notifications");
    await userEvent.click(toggle);
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalled());
    expect(api.updateSettings.mock.calls[0][0].enable_notifications).toBe(false);
  });

  it("rolls a rejected toggle back rather than leaving it flipped", async () => {
    useApi({
      getSettings: settings,
      updateSettings: async () => { throw new Error("settings file is read-only"); },
    });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    // `auto_accept_files` off is the acceptance gate; a UI that shows it on
    // when the backend kept it off is telling the user the wrong thing about
    // whether peers can write to their disk unattended.
    const row = await screen.findByText("Auto-accept incoming files");
    await userEvent.click(row);
    await waitFor(() => expect(toasts().at(-1)?.message).toMatch(/read-only/));
  });

  it("refuses a port outside 1–65535 and restores the previous value", async () => {
    const api = useApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const port = await screen.findByDisplayValue("12345");
    await userEvent.clear(port);
    await userEvent.type(port, "99999");
    await userEvent.tab();

    expect(toasts().at(-1).message).toMatch(/between 1 and 65535/);
    expect(port.value).toBe("12345");
    expect(api.updateSettings).not.toHaveBeenCalled();
  });

  it("saves a valid port change", async () => {
    const api = useApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const port = await screen.findByDisplayValue("12345");
    await userEvent.clear(port);
    await userEvent.type(port, "9000");
    await userEvent.tab();
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalled());
    expect(api.updateSettings.mock.calls[0][0].listen_port).toBe(9000);
  });

  it("commits a renamed display name and tells the shell to refresh", async () => {
    const api = useApi({ getSettings: settings });
    const onIdentityChanged = vi.fn();
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} onIdentityChanged={onIdentityChanged} />);
    const name = await screen.findByLabelText("Display name");
    await userEvent.clear(name);
    await userEvent.type(name, "Maya B");
    await userEvent.tab();
    await waitFor(() => expect(api.setDisplayName).toHaveBeenCalledWith("Maya B"));
    expect(onIdentityChanged).toHaveBeenCalled();
  });

  it("restores the old name when the rename is refused", async () => {
    useApi({ getSettings: settings, setDisplayName: async () => { throw new Error("name is taken"); } });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const name = await screen.findByLabelText("Display name");
    await userEvent.clear(name);
    await userEvent.type(name, "Someone Else");
    await userEvent.tab();
    await waitFor(() => expect(name.value).toBe("Maya"));
  });

  it("does not call the bridge when the name is unchanged", async () => {
    const api = useApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const name = await screen.findByLabelText("Display name");
    await userEvent.click(name);
    await userEvent.tab();
    expect(api.setDisplayName).not.toHaveBeenCalled();
  });
});
