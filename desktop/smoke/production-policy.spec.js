// What the *shipped* page allows, asserted against the bytes a browser receives.
//
// Runs only under the `built` project; `playwright.config.js` excludes this file
// from `dev`, whose policy deliberately differs — Vite's inline preamble needs an
// allowance production must never have.
//
// `csp.test.js` already checks that `csp.js` produces the right string and that
// it matches `tauri.conf.json`. That is a different claim from "the page a
// browser receives carries the tag, and it says what we think" — which is what a
// change to the Vite plugin, the build, or `index.html` can quietly drop.

import { test, expect } from "@playwright/test";

// The production page must not permit inline script. This is the directive that
// makes an injected `<script>` inert, and it is the one that had to be relaxed
// for dev — so it is exactly the one at risk of being relaxed everywhere by a
// change that only ever gets looked at in dev.
//
// Asserted against the served bytes, and paired with a check that the bundle is
// external: `script-src 'self'` is only satisfiable because Vite emits the app
// as a file rather than inline, and if that ever changed the policy would start
// blocking the app itself.
test("the shipped page allows no inline script", async ({ page }) => {
  await page.goto("/");

  const policy = await page.getAttribute(
    'meta[http-equiv="Content-Security-Policy"]',
    "content",
  );
  expect(policy).toContain("script-src 'self'");
  expect(policy).not.toMatch(/script-src[^;]*unsafe-inline/);
  expect(policy).not.toMatch(/script-src[^;]*unsafe-eval/);

  // Every script on the page is external, so nothing depends on an inline
  // allowance the policy does not grant.
  const inlineScripts = await page.$$eval("script", (tags) =>
    tags.filter((tag) => !tag.src && tag.textContent.trim().length > 0).length,
  );
  expect(inlineScripts, "inline <script> blocks in the shipped page").toBe(0);
});
