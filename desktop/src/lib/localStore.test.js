// @vitest-environment jsdom
//
// Integrity-checked localStorage. The checksum is not a MAC and cannot be — the
// algorithm ships in the bundle and a webview holds no secret — so what is
// tested here is what it genuinely buys: corruption is rejected rather than
// adopted, a value cannot be moved between keys, every read is schema-validated,
// and a failure always lands on the caller's default.

import { describe, it, expect, beforeEach } from "vitest";
import { read, write, _envelope } from "./localStore.js";

const anything = () => true;

beforeEach(() => localStorage.clear());

describe("localStore", () => {
  it("round-trips a value", () => {
    write("k", { a: 1, b: "two" });
    expect(read("k", anything)).toEqual({ a: 1, b: "two" });
  });

  it("returns undefined for a key that was never written", () => {
    expect(read("missing", anything)).toBeUndefined();
  });

  it("rejects a damaged entry rather than salvaging what parses", () => {
    write("k", { a: 1 });
    const raw = localStorage.getItem("k");
    localStorage.setItem("k", raw.slice(0, raw.length - 5));
    expect(read("k", anything)).toBeUndefined();
  });

  it("rejects an entry whose body was edited without its checksum", () => {
    write("k", { count: 1 });
    const env = JSON.parse(localStorage.getItem("k"));
    env.d.count = 9999;
    localStorage.setItem("k", JSON.stringify(env));
    expect(read("k", anything)).toBeUndefined();
  });

  it("rejects a perfectly valid envelope written under a different key", () => {
    // The key is part of the checksummed text, so the read-marks blob dropped
    // into the theme slot fails instead of being deserialised as a theme.
    localStorage.setItem("theme", _envelope("party-read", { "s|c": 3 }));
    expect(read("theme", anything)).toBeUndefined();
  });

  it("accepts an envelope written for its own key", () => {
    localStorage.setItem("theme", _envelope("theme", "forest"));
    expect(read("theme", anything)).toBe("forest");
  });

  it("applies the caller's schema on top of the checksum", () => {
    write("k", "not-a-known-theme");
    expect(read("k", (v) => ["dark", "light"].includes(v))).toBeUndefined();
    write("k", "dark");
    expect(read("k", (v) => ["dark", "light"].includes(v))).toBe("dark");
  });

  it("treats a throwing validator as a rejection, not a crash", () => {
    write("k", { a: 1 });
    expect(read("k", () => { throw new Error("bad"); })).toBeUndefined();
  });

  it("migrates a pre-envelope value once, then reads it back checksummed", () => {
    localStorage.setItem("theme", "forest");
    expect(read("theme", (v) => v === "forest", (raw) => raw)).toBe("forest");
    // Rewritten in envelope form, so the legacy branch is not needed again.
    expect(JSON.parse(localStorage.getItem("theme"))).toMatchObject({ v: 1, d: "forest" });
    expect(read("theme", (v) => v === "forest")).toBe("forest");
  });

  it("does not migrate a legacy value the schema rejects", () => {
    localStorage.setItem("theme", "chartreuse");
    expect(read("theme", (v) => v === "forest", (raw) => raw)).toBeUndefined();
    expect(localStorage.getItem("theme")).toBe("chartreuse");
  });

  it("does not fall back to the legacy path for a *tampered* envelope", () => {
    // Otherwise the checksum would be trivially bypassed by breaking it.
    const env = JSON.parse(_envelope("k", { a: 1 }));
    env.c = "00000000";
    localStorage.setItem("k", JSON.stringify(env));
    expect(read("k", anything, (raw) => JSON.parse(raw))).toBeUndefined();
  });

  it("survives storage being unavailable", () => {
    const real = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() { throw new Error("blocked by policy"); },
    });
    expect(() => write("k", 1)).not.toThrow();
    expect(read("k", anything)).toBeUndefined();
    Object.defineProperty(globalThis, "localStorage", real);
  });

  it("survives a value that cannot be serialised", () => {
    const cyclic = {};
    cyclic.self = cyclic;
    expect(() => write("k", cyclic)).not.toThrow();
    expect(read("k", anything)).toBeUndefined();
  });
});
