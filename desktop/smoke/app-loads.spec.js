// The app comes up, paints, and reports nothing broken. Deliberately shallow: it
// notices that the app is *on* (the bug it was written for blanked every screen
// while every other gate stayed green). Each test asserts a ladder, so a failure
// says which stage broke:
//
//   1. the document loaded and carries its CSP
//   2. React mounted (`#root` has children)
//   3. the app shell rendered (a known landmark is visible)
//   4. nothing was refused, thrown, or logged as an error on the way
//
// A blocked script trips rung 2 first, which alone says nothing about why — so
// the `problems` fixture attaches everything the page reported to *every* failure.

import { test as base, expect } from "@playwright/test";

/// A page under observation, plus everything it complained about.
/// `securitypolicyviolation` names the directive and blocked URI (easier to spot
/// than a console line); it is installed by an init script, so it sees violations
/// from the first inline block in `<head>`.
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

    // On any failure, report what the page said: attached for the HTML report
    // and printed for the CI log.
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

/// Assert the page reported nothing wrong, one assertion per channel so the
/// failure names it.
function expectNoProblems(problems) {
  expect(problems.csp, "content-security-policy violations").toEqual([]);
  expect(problems.pageErrors, "uncaught exceptions").toEqual([]);
  expect(problems.consoleErrors, "console errors").toEqual([]);
  expect(problems.failedRequests, "failed requests").toEqual([]);
}

/// The policy read off the served HTML, not `csp.js` (which `csp.test.js`
/// checks): a change to the Vite plugin, the build or `index.html` could drop the tag.
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

    // (2) React mounted: "the bundle never ran", apart from "rendered nothing"
    await expect(page.locator("#root > *")).toHaveCount(1);

    // (3) the shell is on screen, not an empty div or an error boundary
    await expect(
      page.getByRole("navigation", { name: "Primary" }),
    ).toBeVisible();
    await expect(page.locator("#main-content")).toBeVisible();

    // (4) nothing complained
    expectNoProblems(problems);
  });

  // A new user's first screen: a different tree, rendered without the shell
  // until a password is set. The `bridge.js` mock reads its auth state from `?mock=`.
  test("the first-run password screen paints", async ({ page, problems }) => {
    await page.goto("/?mock=set_password");

    await expect(page.locator("#root > *")).toHaveCount(1);
    await expect(
      page.getByRole("button", { name: "Create identity" }),
    ).toBeVisible();
    // A real label, not a placeholder that vanishes as you type.
    await expect(page.getByLabel("New password")).toBeVisible();

    expectNoProblems(problems);
  });
});
