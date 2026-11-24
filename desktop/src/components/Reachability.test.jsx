// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const bridge = vi.hoisted(() => ({ api: {}, real: null }));
vi.mock("../lib/bridge.js", async (importOriginal) => {
  const actual = await importOriginal();
  bridge.real = actual.api;
  return { ...actual, api: bridge.api };
});

import { Reachability } from "./Reachability.jsx";
import { stubApi } from "../test/render.jsx";

function installApi(overrides) {
  const api = stubApi(bridge.real, overrides);
  Object.keys(bridge.api).forEach((k) => delete bridge.api[k]);
  Object.assign(bridge.api, api);
  return api;
}

beforeEach(() => installApi({ myAddresses: { local: "192.168.1.9:12345", external: null }, myInviteLink: "chat-p2p://invite/x" }));

describe("Reachability", () => {
  it("states listening in words and offers the address and Stop", async () => {
    const onStop = vi.fn();
    const l = { id: "h1", name: "Listening :12345" };
    render(<Reachability listeners={[l]} locked={false} connected={1} onStopListening={onStop} />);
    await userEvent.click(screen.getByRole("button", { name: /1 connected\. Listening/ }));
    expect(await screen.findByText("192.168.1.9:12345")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: /Stop listening/ }));
    expect(onStop).toHaveBeenCalledWith(l);
  });

  it("starts listening on the configured port when nothing is listening", async () => {
    const api = installApi({ getSettings: { listen_port: 4242 } });
    const onChanged = vi.fn();
    render(<Reachability listeners={[]} locked={false} onChanged={onChanged} />);
    await userEvent.click(screen.getByRole("button", { name: /Not listening/ }));
    await userEvent.click(screen.getByRole("button", { name: /Start listening/ }));
    expect(api.startHost).toHaveBeenCalledWith(4242, "");
    expect(onChanged).toHaveBeenCalled();
  });

  it("puts the new-peer gate behind a labelled switch", async () => {
    const onToggle = vi.fn();
    render(<Reachability listeners={[]} locked onToggleLock={onToggle} />);
    await userEvent.click(screen.getByRole("button", { name: /Closed to new peers/ }));
    const sw = screen.getByRole("switch", { name: /Accept new peers/ });
    expect(sw.checked).toBe(false);
    await userEvent.click(sw);
    expect(onToggle).toHaveBeenCalled();
  });
});
