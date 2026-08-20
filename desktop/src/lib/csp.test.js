    it("the shipped policy allows no inline script", () => {
        // This is the directive that makes an injected <script> inert. If it
        // ever gains 'unsafe-inline', the CSP has stopped doing the main thing
        // it is here for.
        expect(directives(cspPolicy())["script-src"]).toEqual(["'self'"]);
        expect(directives(cspPolicy({ forHeader: true }))["script-src"]).toEqual(["'self'"]);
    });

    it("allows inline script in dev, because Vite requires it", () => {
        // @vitejs/plugin-react injects its React Refresh preamble as an inline
        // module script. Blocking it leaves the window blank with nothing on
        // screen to explain why — which is exactly what happened when the meta
        // tag was first added, and only in `tauri dev`, because Tauri's CSP
        // header cannot apply to a page the Vite server delivers.
        expect(directives(cspPolicy({ dev: true }))["script-src"]).toContain("'unsafe-inline'");
    });

// The Content Security Policy, and the drift guard that keeps the injected meta
// tag and the Tauri shell's header saying the same thing. When both are present
// the browser enforces their intersection, so a meta tag stricter than the
// header breaks IPC in the packaged app while looking fine in dev.

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { cspPolicy, cspMetaTag } from "./csp.js";

const here = dirname(fileURLToPath(import.meta.url));
const tauriConf = JSON.parse(
  readFileSync(resolve(here, "../../src-tauri/tauri.conf.json"), "utf8"),
);

function directives(policy) {
  return Object.fromEntries(
    policy.split(";").map((d) => {
      const [name, ...values] = d.trim().split(/\s+/);
      return [name, values];
    }),
  );
}

describe("cspPolicy", () => {
  it("blocks inline and eval'd script — the directive that actually matters", () => {
    const d = directives(cspPolicy());
    expect(d["script-src"]).toEqual(["'self'"]);
    expect(d["script-src"]).not.toContain("'unsafe-inline'");
    expect(d["script-src"]).not.toContain("'unsafe-eval'");
  });

  it("allows nothing off-host by default", () => {
    // This app makes no third-party requests at all: a font or analytics fetch
    // on startup would leak that the user runs a secure messenger, and to whom.
    expect(directives(cspPolicy())["default-src"]).toEqual(["'self'"]);
  });

  it("allows data: images, which file previews need", () => {
    // `file_preview` returns base64 rather than a path.
    expect(directives(cspPolicy())["img-src"]).toContain("data:");
  });

  it("reaches only the Tauri IPC endpoints", () => {
    expect(directives(cspPolicy())["connect-src"]).toEqual([
      "'self'", "ipc:", "http://ipc.localhost",
    ]);
  });

  it("shuts the remaining classic holes", () => {
    const d = directives(cspPolicy());
    expect(d["object-src"]).toEqual(["'none'"]);
    expect(d["base-uri"]).toEqual(["'self'"]);
    expect(d["form-action"]).toEqual(["'none'"]);
  });

  it("never ships the dev server's localhost exception", () => {
    expect(cspPolicy()).not.toMatch(/localhost:5173/);
    expect(cspPolicy({ dev: true })).toMatch(/ws:\/\/localhost:5173/);
    // Dev widens exactly two directives and no others: connect-src for the HMR
    // socket, script-src for Vite's inline preamble. Everything else must match
    // what ships, or dev stops resembling production in ways that hide problems
    // until release.
    const dev = directives(cspPolicy({ dev: true }));
    const prod = directives(cspPolicy());
    for (const name of Object.keys(prod)) {
      if (name === "connect-src" || name === "script-src") continue;
      expect(dev[name]).toEqual(prod[name]);
    }
    expect(dev["default-src"]).toEqual(["'self'"]);
  });

  it("keeps frame-ancestors out of the meta form, where it is ignored", () => {
    // A browser drops `frame-ancestors` from a meta tag and warns about it.
    expect(cspPolicy()).not.toMatch(/frame-ancestors/);
    expect(cspPolicy({ forHeader: true })).toMatch(/frame-ancestors 'none'/);
  });
});

describe("cspMetaTag", () => {
  it("is a well-formed http-equiv tag", () => {
    expect(cspMetaTag()).toBe(
      `<meta http-equiv="Content-Security-Policy" content="${cspPolicy()}">`,
    );
  });

  it("carries no quote that would break out of the content attribute", () => {
    expect(cspPolicy({ dev: true })).not.toMatch(/"/);
  });
});

describe("tauri.conf.json", () => {
  it("still sets a CSP at all", () => {
    expect(tauriConf.app?.security?.csp).toBeTruthy();
  });

  it("matches the policy this module generates", () => {
    // Editing one and forgetting the other is the failure this catches: the
    // header and the meta tag intersect, so they must agree.
    expect(tauriConf.app.security.csp).toBe(cspPolicy({ forHeader: true }));
  });
});
