// Helpers shared by the component tests.
//
// The components talk to the backend through one module (`lib/bridge.js`), so
// every test that needs a controlled backend replaces exactly that. The helpers
// here keep the replacement honest: `bridgeStub` starts from the *real* module,
// so a test can only stub commands that actually exist, and a command renamed in
// `bridge.js` without its callers updated shows up as a failing test rather than
// as an `invoke` that silently no-ops in production.

import { vi } from "vitest";

/// Build a stub `api` from the real one: every exported command becomes a
/// `vi.fn()` resolving to `undefined`, then `overrides` replaces the ones this
/// test cares about.
///
/// Taking the shape from the real module is the point. A hand-written `{ ... }`
/// stub keeps passing after a command is removed or renamed, which is exactly
/// the class of breakage these tests exist to catch.
export function stubApi(realApi, overrides = {}) {
  const api = {};
  for (const key of Object.keys(realApi)) {
    api[key] = vi.fn(async () => undefined);
  }
  for (const [key, impl] of Object.entries(overrides)) {
    if (!(key in api)) {
      throw new Error(
        `stubApi: "${key}" is not a bridge command. Either the test is stale or ` +
          `the command was renamed in lib/bridge.js.`,
      );
    }
    api[key] = typeof impl === "function" ? vi.fn(impl) : vi.fn(async () => impl);
  }
  return api;
}

/// A conversation in the shape `chatToContact` produces, for ChatPane tests.
export function contactFixture(over = {}) {
  return {
    id: "chat-1",
    name: "Alice",
    state: "connected",
    trust: "verified",
    fingerprint: "a".repeat(64),
    address: "aaaaaaaaaaaaaaaa…",
    placeholder: false,
    kind: "dm",
    transport: "direct",
    relay: false,
    members: 0,
    typing: false,
    messages: [],
    ...over,
  };
}

/// A conversation-list row in the shape `summaryToConv` produces.
export function convFixture(over = {}) {
  return {
    id: "chat-1",
    name: "Alice",
    last: "hello",
    lastT: "12:00",
    typing: false,
    kind: "dm",
    transport: "direct",
    relay: false,
    state: "connected",
    trust: "verified",
    unread: 0,
    placeholder: false,
    ...over,
  };
}
