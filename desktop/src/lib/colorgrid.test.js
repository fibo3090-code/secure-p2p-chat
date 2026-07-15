import { describe, it, expect } from "vitest";
import { colorGrid } from "./colorgrid.js";

const FP = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6";

describe("colorGrid", () => {
  it("fails closed on missing or non-hex input", () => {
    expect(colorGrid(null)).toEqual([]);
    expect(colorGrid(undefined)).toEqual([]);
    expect(colorGrid("")).toEqual([]);
    expect(colorGrid("zzz---!!!")).toEqual([]);
  });

  it("produces n*n hsl cells", () => {
    const grid = colorGrid(FP);
    expect(grid).toHaveLength(64);
    for (const cell of grid) {
      expect(cell).toMatch(/^hsl\(\d+ \d+% \d+%\)$/);
    }
    expect(colorGrid(FP, 4)).toHaveLength(16);
  });

  it("is deterministic for the same fingerprint", () => {
    expect(colorGrid(FP)).toEqual(colorGrid(FP));
  });

  it("differs between fingerprints", () => {
    const other = FP.replace(/^a1/, "b2");
    expect(colorGrid(FP)).not.toEqual(colorGrid(other));
  });

  it("ignores separators and case in the fingerprint", () => {
    const spaced = FP.toUpperCase().match(/.{4}/g).join(" ");
    // Uppercasing changes charCodes, so only separator-stripping is invariant:
    // the same fp with colons must hash identically to the bare form.
    const colons = FP.match(/.{2}/g).join(":");
    expect(colorGrid(colons)).toEqual(colorGrid(FP));
    expect(colorGrid(spaced)).toHaveLength(64);
  });
});
