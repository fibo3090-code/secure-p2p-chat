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
});
