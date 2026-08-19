import { describe, it, expect, beforeEach, vi } from "vitest";
import { THEMES, loadTheme, saveTheme } from "./themes.js";

const store = new Map();
vi.stubGlobal("localStorage", {
  getItem: (k) => (store.has(k) ? store.get(k) : null),
  setItem: (k, v) => store.set(k, String(v)),
  removeItem: (k) => store.delete(k),
});

beforeEach(() => store.clear());

describe("theme registry", () => {
  it("exposes the five canonical themes", () => {
    expect(THEMES.map((t) => t.id).sort()).toEqual(
      ["dark", "forest", "light", "midnight", "rose"].sort(),
    );
  });

  it("defaults to dark when nothing is saved", () => {
    expect(loadTheme()).toBe("dark");
  });

  it("round-trips a saved theme", () => {
    saveTheme("forest");
    expect(loadTheme()).toBe("forest");
  });

  it("falls back to dark for an unknown saved value", () => {
    saveTheme("not-a-theme");
    expect(loadTheme()).toBe("dark");
  });

  it("keeps a theme chosen before the entry was checksummed", () => {
    // Existing installs stored the bare id. Losing everyone's theme on upgrade
    // would be a silly way to pay for an integrity check.
    store.set("p2pem.theme", "forest");
    expect(loadTheme()).toBe("forest");
    // And it is rewritten in the new form, so the migration happens once.
    expect(store.get("p2pem.theme")).toMatch(/"v":1/);
  });

  it("falls back to dark when the stored entry has been damaged", () => {
    saveTheme("rose");
    store.set("p2pem.theme", store.get("p2pem.theme").replace("rose", "forest"));
    // The body no longer matches its checksum, so it is rejected outright
    // rather than half-applied.
    expect(loadTheme()).toBe("dark");
  });
});
