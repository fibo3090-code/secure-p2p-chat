import { describe, it, expect } from "vitest";
import { parsePort, folderOf } from "./parse.js";

describe("parsePort", () => {
  it("accepts the full valid range", () => {
    expect(parsePort("1")).toBe(1);
    expect(parsePort("12345")).toBe(12345);
    expect(parsePort("65535")).toBe(65535);
    expect(parsePort(" 9000 ")).toBe(9000);
  });

  // The whole point: these used to become 12345 silently, so the app connected
  // somewhere the user never asked for.
  it("rejects anything that is not a port instead of falling back", () => {
    for (const bad of ["", "   ", "0", "65536", "99999", "abc", "80abc", "-1", "1.5", "1e3", null, undefined]) {
      expect(parsePort(bad), `${JSON.stringify(bad)} must not parse`).toBeNull();
    }
  });
});

describe("folderOf", () => {
  it("names the containing folder for Windows paths", () => {
    expect(folderOf("C:\\Users\\me\\Documents\\P2PEM\\report.pdf")).toBe("P2PEM");
    expect(folderOf("C:\\Downloads\\a.txt")).toBe("Downloads");
  });

  it("names the containing folder for POSIX paths", () => {
    expect(folderOf("/home/me/Downloads/a.txt")).toBe("Downloads");
    expect(folderOf("/home/me/received files/a.txt")).toBe("received files");
  });

  it("returns nothing to name when there is no folder", () => {
    expect(folderOf("a.txt")).toBe("");
    expect(folderOf("")).toBe("");
    expect(folderOf(null)).toBe("");
    expect(folderOf(undefined)).toBe("");
    expect(folderOf("/a.txt")).toBe("");
  });
});
