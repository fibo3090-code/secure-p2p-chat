import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { RISKY_EXTENSIONS, fileKind, splitName } from "./fileKind.js";

const here = dirname(fileURLToPath(import.meta.url));

describe("fileKind", () => {
  it("mirrors the bridge's list of extensions that execute", () => {
    const rust = readFileSync(resolve(here, "../../src-tauri/src/commands/chats.rs"), "utf8");
    const body = rust.match(/const RISKY_EXTENSIONS: &\[&str\] = &\[([\s\S]*?)\];/)[1];
    const fromRust = [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect([...RISKY_EXTENSIONS].sort()).toEqual([...fromRust].sort());
  });

  it("judges a double extension by the final one", () => {
    expect(fileKind("holiday.jpg.exe")).toMatchObject({ risky: true, label: "Program", ext: "exe" });
    expect(splitName("holiday.jpg.exe")).toEqual({ stem: "holiday.jpg", ext: "exe" });
  });

  it("names ordinary files without alarm", () => {
    expect(fileKind("roadmap.pdf")).toMatchObject({ risky: false, label: "PDF" });
    expect(fileKind("photo.JPG")).toMatchObject({ risky: false, label: "Image" });
    expect(fileKind("README")).toMatchObject({ risky: false, label: "File", ext: "" });
    expect(fileKind(".bashrc").risky).toBe(false);
  });
});
