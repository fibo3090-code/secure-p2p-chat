// Design-token drift guard.
//
// `design/tokens.json` is the source of record for the brand accent, the
// per-theme surface ramps, and the semantic colours. Nothing generates the CSS
// from it — the two are kept in sync by hand — so this test is what makes drift
// fail CI instead of going unnoticed for a release or two (which is exactly
// what happened before: the docs described a palette the app never shipped).
//
// This replaces the equivalent Rust guard that lived in the egui theming module
// (`client/src/gui/styling.rs`'s `token_drift_tests`) and was removed with it.
// The shipped consumers of these tokens are now the desktop CSS and the TUI's
// brand accent, so that is what gets checked.
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const read = (p) => readFileSync(resolve(repo, p), "utf8");

const tokens = JSON.parse(read("design/tokens.json"));
const css = read("desktop/src/app-system.css") + "\n" + read("desktop/src/themes.css");

/**
 * Every declaration of a CSS custom property, as a list of values in source
 * order. A property is declared once per theme block, so `--accent` yields one
 * entry per theme.
 */
function declarations(prop) {
  const re = new RegExp(`--${prop}\\s*:\\s*([^;}]+)`, "g");
  return [...css.matchAll(re)].map((m) => m[1].trim().toLowerCase());
}

/** The custom properties declared inside one `[data-theme="x"]` block. */
function themeBlock(name) {
  // The base dark/light blocks are written as :root / [data-theme="light"];
  // the rest are :root[data-theme="..."]. Match either shape.
  const re = new RegExp(`\\[data-theme=["']${name}["']\\][^{]*\\{([^}]*)\\}`, "g");
  const bodies = [...css.matchAll(re)].map((m) => m[1]);
  const out = {};
  for (const body of bodies) {
    for (const [, prop, value] of body.matchAll(/--([\w-]+)\s*:\s*([^;]+)/g)) {
      out[prop] = value.trim().toLowerCase();
    }
  }
  return out;
}

describe("design tokens", () => {
  it("declares the brand accent as the default", () => {
    // The first `--accent` declaration is the root default and must be the
    // brand's flat mid-tone.
    expect(declarations("accent")[0]).toBe(tokens.brand.flatAccent.toLowerCase());
  });

  it.each(["midnight", "forest", "rose"])(
    "%s theme matches its tokens",
    (name) => {
      const block = themeBlock(name);
      const t = tokens.themes[name];
      for (const key of ["bg", "s1", "s2", "s3", "s4", "text", "dim"]) {
        expect(block[key], `${name} --${key}`).toBe(t[key].toLowerCase());
      }
      expect(block.accent, `${name} --accent`).toBe(t.accent.toLowerCase());
      expect(block["accent-ink"], `${name} --accent-ink`).toBe(t.accentInk.toLowerCase());
    },
  );

  it("ships every semantic colour exactly as specified", () => {
    for (const [name, hex] of Object.entries(tokens.semantic)) {
      expect(declarations(name), `--${name}`).toContain(hex.toLowerCase());
    }
  });

  it("uses the specified type stack", () => {
    // Compare font families by name; the fallback chains are formatted
    // differently in CSS than in JSON.
    for (const family of ["IBM Plex Sans", "Space Grotesk", "IBM Plex Mono"]) {
      expect(css).toContain(family);
    }
  });

  it("keeps the theme registry aligned with the token file", () => {
    // A theme present in one and missing from the other means a user can pick
    // a theme with no tokens behind it (or vice versa).
    const cssThemes = new Set(
      [...css.matchAll(/\[data-theme=["']([\w-]+)["']\]/g)].map((m) => m[1]),
    );
    // "dark" is the :root default rather than a data-theme block.
    cssThemes.add("dark");
    expect([...cssThemes].sort()).toEqual(Object.keys(tokens.themes).sort());
  });
});
