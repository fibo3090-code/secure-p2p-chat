// @vitest-environment jsdom
//
// The invoke contract, checked from the side that sends it.
//
// Tauri binds invoke arguments by exact key: a key the Rust command does not
// name is ignored, and the command then fails or quietly does nothing ("sent
// but never arrives" and "verify does nothing" were both this). The Rust IPC
// tests drive every command with payloads written out by hand, which only
// proves the payloads they wrote. This runs every real wrapper in `realApi`
// against a recording `invoke` and holds what it actually sends to two things:
//
//   1. the command exists — it is registered in `invoke_handler()` (lib.rs);
//   2. its exact key set is one the Rust tests send (tests.rs), so the Rust
//      side has proved those keys bind.
//
// Renaming a key on either side alone now fails here.

import { describe, it, expect, vi } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const sent = vi.hoisted(() => {
  // `bridge.js` picks the real API at import time when this is present.
  globalThis.window.__TAURI_INTERNALS__ = {};
  return [];
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd, args) => {
    sent.push([cmd, args]);
    return null;
  }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

const here = dirname(fileURLToPath(import.meta.url));
const rust = (p) => readFileSync(resolve(here, "../../src-tauri/src", p), "utf8");

// Top-level keys of the object literal starting at `src[open]` (a `{`).
function topLevelKeys(src, open) {
  const keys = [];
  let depth = 0;
  for (let i = open; i < src.length; i++) {
    const c = src[i];
    if (c === "{" || c === "[") depth++;
    else if (c === "}" || c === "]") {
      depth--;
      if (depth === 0) return keys;
    } else if (c === '"' && depth === 1) {
      const end = src.indexOf('"', i + 1);
      const word = src.slice(i + 1, end);
      if (/^\s*:/.test(src.slice(end + 1))) keys.push(word);
      i = end;
    } else if (c === '"') {
      i = src.indexOf('"', i + 1); // a string value one level down
    }
  }
  return keys;
}

// Every ("command", json!({ … })) the Rust tests send: command → key sets.
function rustPayloads() {
  const src = rust("tests.rs");
  const byCommand = new Map();
  const re = /"([a-z_]+)",\s*json!\(\s*\{/g;
  for (let m; (m = re.exec(src)); ) {
    const keys = topLevelKeys(src, m.index + m[0].length - 1).sort().join(",");
    if (!byCommand.has(m[1])) byCommand.set(m[1], new Set());
    byCommand.get(m[1]).add(keys);
  }
  return byCommand;
}

function registeredCommands() {
  const src = rust("lib.rs");
  const block = src.slice(src.indexOf("generate_handler!["));
  return new Set(
    [...block.slice(0, block.indexOf("]")).matchAll(/commands::\w+::(\w+)/g)].map((m) => m[1]),
  );
}

async function everyCall() {
  const { api } = await import("./bridge.js");
  const calls = [];
  for (const [name, fn] of Object.entries(api)) {
    sent.length = 0;
    // Distinct placeholder values, enough for the longest wrapper.
    await fn("a", "b", "c", "d", "e");
    expect(sent, `${name} did not invoke exactly once`).toHaveLength(1);
    const [cmd, args] = sent[0];
    calls.push({ name, cmd, keys: Object.keys(args || {}).sort().join(",") });
  }
  return calls;
}

describe("the invoke contract", () => {
  it("uses the real wrappers here, not the dev mock", async () => {
    const { api } = await import("./bridge.js");
    sent.length = 0;
    await api.listConversations();
    expect(sent).toEqual([["list_conversations", undefined]]);
  });

  it("sends only commands the backend registers", async () => {
    const registered = registeredCommands();
    expect(registered.size).toBeGreaterThan(60);
    const unknown = (await everyCall()).filter((c) => !registered.has(c.cmd));
    expect(unknown, "bridge.js invokes a command lib.rs does not register").toEqual([]);
  });

  it("sends each command exactly the keys a Rust test proves bind", async () => {
    const proven = rustPayloads();
    const unproven = (await everyCall())
      .filter((c) => c.keys !== "")
      .filter((c) => !proven.get(c.cmd)?.has(c.keys))
      .map((c) => `${c.name} → ${c.cmd}({${c.keys}}); Rust sends ${
        JSON.stringify([...(proven.get(c.cmd) || [])])}`);
    expect(unproven).toEqual([]);
  });
});
