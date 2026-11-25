// @vitest-environment jsdom
// (bridge.js, which owns the extension list, reads `location` on import.)
import { describe, it, expect } from "vitest";
import { RISKY_EXTENSIONS, fileKind, splitName } from "./fileKind.js";

describe("fileKind", () => {
  // One list, the bridge's: a card that names a file "document" while the
  // click warns "this is a program" is the confusion this module removes.
  it("calls every extension the bridge refuses to open a program", () => {
    expect(RISKY_EXTENSIONS.size).toBeGreaterThan(40);
    for (const ext of RISKY_EXTENSIONS) {
      expect(fileKind(`x.${ext}`), ext).toMatchObject({ risky: true, label: "Program" });
    }
    // Ones a shorter copy of the list once missed.
    for (const name of ["setup.dmg", "invoice.docm", "page.html", "disk.iso"]) {
      expect(fileKind(name).risky, name).toBe(true);
    }
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
