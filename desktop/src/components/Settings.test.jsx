// @vitest-environment jsdom
//
// Settings. Two behaviours here are more than cosmetic: a toggle the backend
// rejects has to roll *back* (leaving it flipped tells the user a setting is on
// that is not), and the identity-backup row has to state plainly whether a
// backup exists — losing the identity file is the one failure this app cannot
// undo, and an Export button with no indication either way says nothing.

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
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

function installApi(overrides) {
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

beforeEach(() => { clearToasts(); installApi({ getSettings: settings }); });
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
    installApi({ getSettings: { ...settings, identity_backed_up_at: 1700000000000 } });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    expect(await screen.findByText("Backed up")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Export again/ })).toBeTruthy();
  });

  it("persists a toggle through the bridge", async () => {
    const api = installApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const toggle = await screen.findByText("Desktop notifications");
    await userEvent.click(toggle);
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalled());
    expect(api.updateSettings.mock.calls[0][0].enable_notifications).toBe(false);
  });

  it("rolls a rejected toggle back rather than leaving it flipped", async () => {
    installApi({
      getSettings: settings,
      updateSettings: async () => { throw new Error("settings file is read-only"); },
    });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    // `auto_accept_files` off is the acceptance gate; a UI that shows it on
    // when the backend kept it off is telling the user the wrong thing about
    // whether peers can write to their disk unattended.
    const row = await screen.findByText("Auto-accept incoming files");
    const toggle = row.closest('[role="switch"]');
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    await userEvent.click(row);
    await waitFor(() => expect(toasts().at(-1)?.message).toMatch(/read-only/));
    // The title's promise: the switch shows the backend's value again.
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("false"));
  });

  // Every switch is the same component, so the bug to catch is wiring: a
  // label that writes another setting ("Reconnect contacts" flipping mDNS,
  // which announces you on the LAN). Each one must write its own key, the
  // flipped value, and nothing else.
  it.each([
    ["Desktop notifications", "enable_notifications"],
    ["Send typing indicators", "enable_typing_indicators"],
    ["Host automatically on startup", "auto_host_on_startup"],
    ["UPnP port mapping", "enable_upnp"],
    ["Reconnect contacts on startup", "auto_connect"],
    ["LAN peer discovery (mDNS)", "enable_mdns"],
    ["Auto-accept incoming files", "auto_accept_files"],
  ])("%s writes %s and only that", async (label, key) => {
    const api = installApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const toggle = (await screen.findByText(label)).closest('[role="switch"]');
    expect(toggle.getAttribute("aria-checked")).toBe(String(settings[key]));
    await userEvent.click(toggle);
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalledTimes(1));
    const sent = api.updateSettings.mock.calls[0][0];
    const changed = Object.keys(sent).filter((k) => sent[k] !== settings[k]);
    expect(changed).toEqual([key]);
    expect(sent[key]).toBe(!settings[key]);
    await waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe(String(!settings[key])));
  });

  it("refuses a port outside 1–65535 and restores the previous value", async () => {
    const api = installApi({ getSettings: settings });
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
    const api = installApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const port = await screen.findByDisplayValue("12345");
    await userEvent.clear(port);
    await userEvent.type(port, "9000");
    await userEvent.tab();
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalled());
    expect(api.updateSettings.mock.calls[0][0].listen_port).toBe(9000);
  });

  it("commits a renamed display name and tells the shell to refresh", async () => {
    const api = installApi({ getSettings: settings });
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
    installApi({ getSettings: settings, setDisplayName: async () => { throw new Error("name is taken"); } });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const name = await screen.findByLabelText("Display name");
    await userEvent.clear(name);
    await userEvent.type(name, "Someone Else");
    await userEvent.tab();
    await waitFor(() => expect(name.value).toBe("Maya"));
  });

  it("does not call the bridge when the name is unchanged", async () => {
    const api = installApi({ getSettings: settings });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const name = await screen.findByLabelText("Display name");
    await userEvent.click(name);
    await userEvent.tab();
    expect(api.setDisplayName).not.toHaveBeenCalled();
  });

  it("deletes everything only with the password and the typed word", async () => {
    const api = installApi({ getSettings: settings, deleteAllData: undefined });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    await userEvent.click(await screen.findByText(/Delete…/));
    const go = screen.getByRole("button", { name: /Delete everything/ });
    expect(go.disabled).toBe(true);

    await userEvent.type(screen.getByPlaceholderText("Your password"), "correct horse battery");
    expect(go.disabled).toBe(true);
    await userEvent.type(screen.getByLabelText("Type DELETE to confirm"), "delete");
    expect(go.disabled).toBe(true); // the word is case-sensitive
    await userEvent.clear(screen.getByLabelText("Type DELETE to confirm"));
    await userEvent.type(screen.getByLabelText("Type DELETE to confirm"), "DELETE");
    await userEvent.click(go);
    expect(api.deleteAllData).toHaveBeenCalledWith("correct horse battery", "DELETE");
    expect(await screen.findByText(/Everything has been deleted/)).toBeTruthy();
  });

  // Changing the one password the app has. The floor is the bridge's real
  // rule, a mismatch must not reach the bridge, a refusal (wrong current
  // password) stays on screen, and typed passwords do not outlive the dialog
  // — it stays mounted, so leaving them in state left them in the DOM.
  it("changes the password only when the new one is valid and confirmed", async () => {
    const changePassword = vi.fn(async (current) => {
      if (current !== "the-current-password") throw "Current password is incorrect";
    });
    installApi({ getSettings: settings, changePassword });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const open = async () => {
      await screen.findAllByRole("button", { name: /^Change$/ });
      const passwordRow = screen.getByText(/Unlocks this device's identity file/).closest(".set-row");
      await userEvent.click(within(passwordRow).getByRole("button", { name: /^Change$/ }));
      return {
        current: screen.getByPlaceholderText("Current password"),
        next: screen.getByPlaceholderText(/New password \(at least 12 characters\)/),
        confirm: screen.getByPlaceholderText("Repeat the new password"),
        submit: screen.getByRole("button", { name: "Change password" }),
      };
    };

    let f = await open();
    await userEvent.type(f.current, "the-current-password");
    await userEvent.type(f.next, "short");
    expect(await screen.findByText(/At least 12 characters/)).toBeTruthy();
    expect(f.submit.disabled).toBe(true);
    await userEvent.clear(f.next);
    await userEvent.type(f.next, "a-long-new-password");
    await userEvent.type(f.confirm, "a-different-one!!");
    expect(await screen.findByText(/don't match/)).toBeTruthy();
    expect(f.submit.disabled).toBe(true);
    expect(changePassword).not.toHaveBeenCalled();

    // Closing clears what was typed.
    await userEvent.keyboard("{Escape}");
    f = await open();
    expect([f.current.value, f.next.value, f.confirm.value]).toEqual(["", "", ""]);

    // A refusal is shown and the dialog stays open.
    await userEvent.type(f.current, "a-wrong-password");
    await userEvent.type(f.next, "a-long-new-password");
    await userEvent.type(f.confirm, "a-long-new-password");
    await userEvent.click(f.submit);
    expect(await screen.findByText(/Current password is incorrect/)).toBeTruthy();
    expect(screen.getByRole("dialog")).toBeTruthy();

    await userEvent.clear(f.current);
    await userEvent.type(f.current, "the-current-password");
    await userEvent.click(f.submit);
    await waitFor(() =>
      expect(changePassword).toHaveBeenLastCalledWith("the-current-password", "a-long-new-password"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(toasts().some((t) => t.message.includes("Password changed"))).toBe(true);
  });

  it("exports the identity backup and diagnostics through the bridge", async () => {
    const api = installApi({
      getSettings: settings,
      exportIdentity: "/backups/p2pem-identity-backup.json",
      exportDiagnostics: "/data/diagnostics/bundle-1",
    });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    await userEvent.click(await screen.findByRole("button", { name: /Export now/ }));
    await waitFor(() => expect(api.exportIdentity).toHaveBeenCalled());
    await waitFor(() =>
      expect(toasts().some((t) => t.message.includes("/backups/p2pem-identity-backup.json"))).toBe(true));
    // The settings are re-read so the row can say a backup now exists.
    expect(api.getSettings.mock.calls.length).toBeGreaterThan(1);

    await userEvent.click(screen.getByRole("button", { name: /^Export$/ }));
    await waitFor(() =>
      expect(toasts().some((t) => t.message.includes("/data/diagnostics/bundle-1"))).toBe(true));
  });
});


describe("Settings — what notifications show", () => {
  // PRIV-06: notification text lands in the OS notification store and on the
  // lock screen, outside the app's encryption. The choice is offered, saved
  // through the bridge, and absent while notifications are off.
  it("saves the chosen level and is offered only while notifications are on", async () => {
    const api = installApi({
      getSettings: { ...settings, enable_notifications: true, notification_content: "sender_only" },
      updateSettings: vi.fn(async () => {}),
    });
    const { unmount } = render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    const select = await screen.findByLabelText("What notifications show");
    expect(select.value).toBe("sender_only");
    await userEvent.selectOptions(select, "hidden");
    await waitFor(() => expect(api.updateSettings).toHaveBeenCalledWith(
      expect.objectContaining({ notification_content: "hidden" }),
    ));
    unmount();

    installApi({ getSettings: { ...settings, enable_notifications: false } });
    render(<Settings identity={identity} theme="dark" setTheme={() => {}} />);
    await screen.findByText("Desktop notifications");
    expect(screen.queryByLabelText("What notifications show")).toBeNull();
  });
});
