// Smoke tests: does the assembled application actually come up?
//
// Everything else in this suite tests *pieces*. `csp.test.js` checks the
// policy's contents, the component tests render trees under jsdom, the invoke
// contract compares two lists of names. All of them passed — 15 CI checks green
// — on the change that made the app open to a permanently blank window.
//
// It got through because no gate anywhere started the app and looked at it. The
// break was a `script-src 'self'` meta tag blocking the inline React Refresh
// preamble that `@vitejs/plugin-react` injects in dev: the entry module never
// ran, so the thing that would have rendered an error was the thing that failed
// to load. jsdom cannot catch that — it does not enforce CSP at all — so this
// needs a real browser, and it is the only part of the suite that uses one.
//
// Two servers, because they are genuinely different code paths and only one of
// them broke:
//
//   dev    — Vite's dev server, with HMR and the inline preamble. The path that
//            regressed, and the one CI had never touched.
//   built  — `vite preview` over `dist/`, i.e. the bytes the packaged app
//            embeds, under the production policy with no inline allowance.
//
// Deliberately no Tauri here. `bridge.js` falls back to an in-memory mock when
// `window.__TAURI_INTERNALS__` is absent, so the whole UI is reachable in a
// plain browser — which means this runs headless on Linux, macOS and Windows
// without a webview, a display, or a Rust build.

import { defineConfig } from "@playwright/test";

// The dev port is pinned in `vite.config.js` and `tauri.conf.json`; keep it in
// step with those. The preview port is Vite's own default.
const DEV_PORT = 5173;
const PREVIEW_PORT = 4173;

// `127.0.0.1` rather than `localhost` for the same reason `vite.config.js` pins
// it: which of `::1` and `127.0.0.1` a name resolves to first is a property of
// the machine, and a mismatch here shows up as a blank page — the exact symptom
// these tests exist to distinguish from a real failure.
const DEV_URL = `http://127.0.0.1:${DEV_PORT}`;
const PREVIEW_URL = `http://127.0.0.1:${PREVIEW_PORT}`;

export default defineConfig({
  testDir: "./smoke",
  // Generous: the first navigation waits on a cold Vite dep-optimisation pass.
  timeout: 45_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  // No retries. A smoke test that passes on the second attempt is telling you
  // something, and burying it defeats the point of the job.
  retries: 0,
  workers: process.env.CI ? 1 : undefined,
  // The HTML report is what the CI job uploads on failure: a blank-page bug
  // is near-impossible to diagnose from a log line, and the report carries the
  // screenshot and the recorded console/CSP output together.
  reporter: process.env.CI
    ? [["github"], ["list"], ["html", { open: "never" }]]
    : [["list"]],
  use: {
    // Nothing here is timing-sensitive enough to want video, and traces on
    // first retry are moot with retries at zero.
    screenshot: "only-on-failure",
  },

  webServer: [
    {
      command: `npx vite --port ${DEV_PORT} --strictPort --host 127.0.0.1`,
      url: DEV_URL,
      reuseExistingServer: !process.env.CI,
      timeout: 90_000,
      stdout: "pipe",
      stderr: "pipe",
    },
    {
      // `dist/` is built by the `smoke` npm script before this runs, so preview
      // always serves the current source rather than the committed artifact.
      command: `npx vite preview --port ${PREVIEW_PORT} --strictPort --host 127.0.0.1`,
      url: PREVIEW_URL,
      reuseExistingServer: !process.env.CI,
      timeout: 90_000,
      stdout: "pipe",
      stderr: "pipe",
    },
  ],

  projects: [
    { name: "dev", use: { baseURL: DEV_URL } },
    { name: "built", use: { baseURL: PREVIEW_URL } },
  ],
});
