// Text contrast, measured from the shipped CSS.
//
// `--faint` is the colour of small text — timestamps, hints, empty states, day
// separators — at 10.5–12.5 px, where WCAG AA asks for 4.5:1. Every theme sat
// between 2.7:1 and 4.1:1 (audit UI-10): legible on a good screen in a dark
// room, not otherwise. This reads each theme's tokens out of the CSS the app
// ships and checks the ratio against every surface that text sits on, so a
// palette tweak cannot quietly undo it.
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const css =
  readFileSync(resolve(here, "../app-system.css"), "utf8") +
  "\n" +
  readFileSync(resolve(here, "../themes.css"), "utf8");

function themeTokens(name) {
  const block = css.match(new RegExp(`:root\\[data-theme="${name}"\\]\\s*\\{([^}]*)\\}`));
  if (!block) throw new Error(`no theme block for ${name}`);
  const tokens = {};
  for (const [, key, value] of block[1].matchAll(/--([a-z0-9-]+)\s*:\s*(#[0-9a-fA-F]{6})/g)) {
    tokens[key] = value;
  }
  return tokens;
}

function luminance(hex) {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const lin = (c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

describe("text contrast", () => {
  it("computes the WCAG ratio (known answers)", () => {
    expect(contrast("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrast("#777777", "#ffffff")).toBeCloseTo(4.48, 2);
  });

  for (const theme of ["dark", "light", "midnight", "forest", "rose"]) {
    it(`${theme}: faint text meets AA (4.5:1) on every surface it sits on`, () => {
      const t = themeTokens(theme);
      for (const surface of ["bg", "s1", "s2"]) {
        expect(t[surface], `${theme} --${surface}`).toBeTruthy();
        const ratio = contrast(t.faint, t[surface]);
        expect(ratio, `${theme}: --faint ${t.faint} on --${surface} ${t[surface]}`).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});

// UI-09: `.toast-region:empty { display: none }` took both live regions
// (role=alert, role=status) out of the accessibility tree while empty, and a
// region that is not rendered when content arrives is not reliably announced —
// the first toast was often lost to a screen reader. A live region must stay
// rendered; an empty one has no size anyway.
describe("live regions", () => {
  it("are never removed from rendering while empty", () => {
    const polish = readFileSync(resolve(here, "../polish.css"), "utf8");
    const rules = [...polish.matchAll(/([^{}]*\.toast-region[^{}]*)\{([^}]*)\}/g)];
    expect(rules.length).toBeGreaterThan(0);
    for (const [, selector, body] of rules) {
      expect(body, `${selector.trim()}`).not.toMatch(/display\s*:\s*none|visibility\s*:\s*hidden/);
    }
  });
});
