// The app comes up, paints, and reports nothing broken while doing it.
//
// This is deliberately shallow. It is not here to test behaviour — the rest of
// the suite does that — it is here to notice that the application is *on*. The
// bug it was written for made every screen blank in `tauri dev` while every
// other gate stayed green, because nothing anywhere had ever loaded the page.
//
// Each test asserts a ladder rather than one thing, so a failure says which
// stage broke:
//
//   1. the document loaded and carries its CSP
//   2. React mounted (`#root` has children)
//   3. the app shell rendered (a known landmark is visible)
//   4. nothing was refused, thrown, or logged as an error on the way
//
// Rung 4 is what catches the CSP class of bug directly, but rung 2 is what
// actually trips first when a script is blocked — so the collected violations
// are attached to *every* failure by the `problems` fixture below. Without
// that, the report says "React did not mount" and leaves you to guess why,
// which is the same dead end the original bug presented.

import { test as base, expect } from "@playwright/test";

/// A page under observation, plus the record of everything it complained about.
///
/// `securitypolicyviolation` is worth listening for on top of the console
/// because it names the directive and the blocked URI. "Refused to execute
/// inline script" in a console dump is easy to skim past; a violation record
/// pointing at `script-src` is not. The listener goes in via an init script, so
/// it is installed before the document's own scripts and sees violations from
/// the very first inline block in `<head>`.
const test = base.extend({
  problems: async ({ page }, use, testInfo) => {
    const problems = {
      csp: [],
      consoleErrors: [],
      pageErrors: [],
      failedRequests: [],
    };

    await page.exposeFunction("__smokeRecordCsp", (violation) => {
      problems.csp.push(violation);
    });
    await page.addInitScript(() => {
      document.addEventListener("securitypolicyviolation", (event) => {
        window.__smokeRecordCsp({
          directive: event.effectiveDirective || event.violatedDirective,
          blocked: event.blockedURI,
          sample: event.sample,
        });
      });
    });

    page.on("console", (message) => {
      if (message.type() === "error") problems.consoleErrors.push(message.text());
    });
    page.on("pageerror", (error) => {
      problems.pageErrors.push(error.stack || String(error));
    });
    page.on("requestfailed", (request) => {
      problems.failedRequests.push(
        `${request.url()} — ${request.failure()?.errorText ?? "unknown"}`,
      );
    });

    await use(problems);

    // Whatever the test failed on, say what the page reported. Attached for the
    // HTML report and printed for the CI log, which is the only one anybody
    // reads when a job goes red.
    if (testInfo.status !== testInfo.expectedStatus) {
      const record = JSON.stringify(problems, null, 2);
      await testInfo.attach("page-problems", {
        body: record,
        contentType: "application/json",
      });
      console.error(`\nthe page reported:\n${record}\n`);
    }
  },
});

/// Assert the page reported nothing wrong.
///
/// One assertion per channel so the failure message names the channel, and
/// Playwright prints the offending array inline rather than needing a rerun.
function expectNoProblems(problems) {
  expect(problems.csp, "content-security-policy violations").toEqual([]);
  expect(problems.pageErrors, "uncaught exceptions").toEqual([]);
  expect(problems.consoleErrors, "console errors").toEqual([]);
  expect(problems.failedRequests, "failed requests").toEqual([]);
}

/// The policy the document actually carries, read off the served HTML rather
/// than out of `csp.js`.
///
/// `csp.test.js` already checks that the module produces the right string and
/// that it matches `tauri.conf.json`. That is a different claim from "the page
/// a browser receives has the tag on it", which is what a change to the Vite
/// plugin, the build, or `index.html` can quietly drop.
async function servedPolicy(page) {
  return page.getAttribute(
    'meta[http-equiv="Content-Security-Policy"]',
    "content",
  );
}

test.describe("the app loads", () => {
  test("the main window paints the app shell", async ({ page, problems }) => {
    await page.goto("/");

    // (1) the document arrived with a policy on it
    expect(await servedPolicy(page)).toContain("default-src 'self'");

    // (2) React mounted. Checked apart from the landmark below so a failure
    // distinguishes "the bundle never ran" from "it ran and rendered nothing".
    await expect(page.locator("#root > *")).toHaveCount(1);

    // (3) the shell is on screen, not an empty div or an error boundary
    await expect(
      page.getByRole("navigation", { name: "Primary" }),
    ).toBeVisible();
    await expect(page.locator("#main-content")).toBeVisible();

    // (4) nothing complained
    expectNoProblems(problems);
  });

  // The first screen a new user ever sees, and a different component tree from
  // the one above — the app blocks all UI until the keystore password is set,
  // so this path renders without the shell at all. `bridge.js`'s mock takes its
  // auth state from `?mock=`.
  test("the first-run password screen paints", async ({ page, problems }) => {
    await page.goto("/?mock=set_password");

    await expect(page.locator("#root > *")).toHaveCount(1);
    await expect(
      page.getByRole("button", { name: "Create identity" }),
    ).toBeVisible();
    await expect(page.getByPlaceholder(/New password/)).toBeVisible();

    expectNoProblems(problems);
  });
});

// The production page must not permit inline script. This is the directive that
// makes an injected `<script>` inert, and it is the one that had to be relaxed
// for dev — so it is exactly the one at risk of being relaxed everywhere by a
// change that only ever gets looked at in dev.
//
// Asserted against the served bytes, and paired with a check that the bundle is
// external: `script-src 'self'` is only satisfiable because Vite emits the app
// as a file rather than inline, and if that ever changed the policy would start
// blocking the app itself.
test("the shipped page allows no inline script", async ({ page }, testInfo) => {
  test.skip(
    testInfo.project.name !== "built",
    "about the production policy; the dev server deliberately differs",
  );

  await page.goto("/");

  const policy = await servedPolicy(page);
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
