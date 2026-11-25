// @vitest-environment jsdom
//
// The dev mock has to offer every command the real bridge does.
//
// `npm run dev` in a plain browser is how UI work gets done here, and outside
// Tauri `api` *is* the mock. A command missing from it does not fail a build or
// a test of its own: the screen that calls it throws a TypeError the first
// time someone clicks, in the browser only, and the reflex is to stop using
// the mock rather than fix it. The component tests cannot catch this either:
// `stubApi` builds its stub from the real `api` object of whichever module it
// imports. So this loads `bridge.js` both ways and compares the two surfaces.

import { describe, it, expect, vi, afterEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

async function load({ tauri }) {
  vi.resetModules();
  if (tauri) window.__TAURI_INTERNALS__ = {};
  else delete window.__TAURI_INTERNALS__;
  return import("./bridge.js");
}

afterEach(() => {
  delete window.__TAURI_INTERNALS__;
});

describe("the dev mock's surface", () => {
  it("offers exactly the commands the real bridge offers", async () => {
    const real = (await load({ tauri: true })).api;
    const mock = (await load({ tauri: false })).api;
    expect(mock).not.toBe(real);

    const realKeys = Object.keys(real).sort();
    const mockKeys = Object.keys(mock).sort();
    // Missing from the mock: a screen that throws in the browser only.
    expect(realKeys.filter((k) => !mockKeys.includes(k))).toEqual([]);
    // Only in the mock: a component written against a command that does not
    // exist, which would silently do nothing in the shipped app.
    expect(mockKeys.filter((k) => !realKeys.includes(k))).toEqual([]);
  });

  it("answers every command with a promise, as invoke does", async () => {
    // A mock command that returns a bare value breaks every `await x.then(..)`
    // and `.catch(..)` at the call site, again only in the browser.
    const mock = (await load({ tauri: false })).api;
    const sync = [];
    for (const [name, fn] of Object.entries(mock)) {
      expect(typeof fn, name).toBe("function");
      let result;
      try {
        result = fn();
      } catch {
        sync.push(name); // threw synchronously instead of rejecting
        continue;
      }
      if (!(result instanceof Promise)) sync.push(name);
      else result.catch(() => {});
    }
    expect(sync).toEqual([]);
  });

  it("boots into the state the URL asks for", async () => {
    // The `?mock=` switches are how the auth and error screens are previewed.
    for (const state of ["ready", "unlock", "set_password", "error"]) {
      window.history.replaceState(null, "", state === "ready" ? "/" : `/?mock=${state}`);
      const mock = (await load({ tauri: false })).api;
      expect((await mock.authStatus()).state).toBe(state);
    }
    window.history.replaceState(null, "", "/");
  });
});
