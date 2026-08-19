// @vitest-environment jsdom
//
// The visual fingerprint. It is compared by eye between two machines, so the
// only properties that matter are: the same fingerprint always draws the same
// grid, and different fingerprints draw different ones.

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { SafetyGrid } from "./SafetyGrid.jsx";

const cells = (container) =>
  [...container.querySelectorAll(".verify-cell")].map((el) => el.style.background);

describe("SafetyGrid", () => {
  it("draws n×n cells", () => {
    const { container } = render(<SafetyGrid fingerprint={"ab".repeat(32)} n={8} />);
    expect(container.querySelectorAll(".verify-cell")).toHaveLength(64);
  });

  it("is deterministic — two machines must draw the same grid", () => {
    const a = render(<SafetyGrid fingerprint={"ab".repeat(32)} n={8} />);
    const b = render(<SafetyGrid fingerprint={"ab".repeat(32)} n={8} />);
    expect(cells(a.container)).toEqual(cells(b.container));
  });

  it("changes when the fingerprint changes", () => {
    const a = render(<SafetyGrid fingerprint={"ab".repeat(32)} n={8} />);
    const b = render(<SafetyGrid fingerprint={"cd".repeat(32)} n={8} />);
    expect(cells(a.container)).not.toEqual(cells(b.container));
  });

  it("fails closed on a missing fingerprint — no grid, not a fabricated one", () => {
    // Drawing *something* for an absent fingerprint would give the user a
    // comparison target that means nothing, which is worse than showing none.
    const { container } = render(<SafetyGrid fingerprint="" n={4} />);
    expect(container.querySelectorAll(".verify-cell")).toHaveLength(0);
  });

  it("honours the requested cell size", () => {
    const { container } = render(<SafetyGrid fingerprint={"ab".repeat(32)} n={4} cell={12} />);
    expect(container.querySelector(".verify-cell").style.width).toBe("12px");
  });
});
