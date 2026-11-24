// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import { TEXT_SIZES, loadTextSize, saveTextSize, applyTextSize } from "./textSize.js";

describe("text size", () => {
  it("defaults, persists, and applies as a document zoom", () => {
    expect(loadTextSize()).toBe("default");
    saveTextSize("large");
    expect(loadTextSize()).toBe("large");
    applyTextSize("large");
    expect(document.documentElement.style.zoom).toBe("1.12");
    applyTextSize("default");
    expect(document.documentElement.style.zoom).toBe("");
  });

  it("ignores an unknown size", () => {
    saveTextSize("huge");
    expect(TEXT_SIZES.map((t) => t.id)).not.toContain("huge");
  });
});
