// Smoke tests: does the assembled app actually come up? Every other test checks
// pieces, and all passed on a change (a `script-src 'self'` meta tag blocking the
// inline React Refresh preamble) that left every screen blank. jsdom does not
// enforce CSP, so this needs a real browser — the only part of the suite that does.
//
// Two servers, because they are different code paths:
//   dev    — Vite's dev server, with HMR and the inline preamble (what regressed).
//   built  — `vite preview` over `dist/`, under the production policy.
//
// No Tauri: `bridge.js` falls back to an in-memory mock without
// `window.__TAURI_INTERNALS__`, so this runs headless with no webview or Rust build.

import { defineConfig } from "@playwright/test";

// Keep DEV_PORT in step with `vite.config.js` and `tauri.conf.json`; the preview
// port is Vite's default.
const DEV_PORT = 5173;
const PREVIEW_PORT = 4173;

// `127.0.0.1`, not `localhost`: which of `::1`/`127.0.0.1` a name resolves to is
// per machine, and a mismatch looks like the blank page these tests hunt for.
const DEV_URL = `http://127.0.0.1:${DEV_PORT}`;
const PREVIEW_URL = `http://127.0.0.1:${PREVIEW_PORT}`;

export default defineConfig({
  testDir: "./smoke",
  // Generous: the first navigation waits on a cold Vite dep-optimisation pass.
  timeout: 45_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  // No retries: a smoke test that passes on the second attempt is telling you something.
  retries: 0,
  workers: process.env.CI ? 1 : undefined,
  // CI uploads the HTML report on failure: it carries the screenshot and the
  // recorded console/CSP output, which a blank-page bug needs.
  reporter: process.env.CI
    ? [["github"], ["list"], ["html", { open: "never" }]]
    : [["list"]],
  use: {
    screenshot: "only-on-failure",
  },

  webServer: [
    {
      // `npm run`, not `npx`: npx can fetch from the registry and run lifecycle
      // scripts when a local binary is missing.
      command: `npm run dev -- --port ${DEV_PORT} --strictPort --host 127.0.0.1`,
      url: DEV_URL,
      reuseExistingServer: !process.env.CI,
      timeout: 90_000,
      stdout: "pipe",
      stderr: "pipe",
    },
    {
      // The `smoke` npm script builds `dist/` first, so this serves current source.
      command: `npm run preview -- --port ${PREVIEW_PORT} --strictPort --host 127.0.0.1`,
      url: PREVIEW_URL,
      reuseExistingServer: !process.env.CI,
      timeout: 90_000,
      stdout: "pipe",
      stderr: "pipe",
    },
  ],

  projects: [
    {
      name: "dev",
      use: { baseURL: DEV_URL },
      // The production policy does not apply to the dev server. Excluded here
      // rather than by a `test.skip`, which reads as a disabled test.
      testIgnore: /production-policy\.spec\.js/,
    },
    { name: "built", use: { baseURL: PREVIEW_URL } },
  ],
});
