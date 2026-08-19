// Suite-wide test setup.
//
// Runs for every test file, including the node-environment ones, so it must not
// assume a DOM exists.

import { afterEach } from "vitest";

// This jsdom build exposes `sessionStorage` but leaves `localStorage` undefined,
// so the storage-backed modules (theme, Communities read marks) would otherwise
// be tested against nothing at all — every read taking the "storage unavailable"
// branch and every test passing for the wrong reason. A minimal in-memory
// implementation makes the real code path run.
if (typeof window !== "undefined" && !window.localStorage) {
  const store = new Map();
  const localStorage = {
    getItem: (k) => (store.has(String(k)) ? store.get(String(k)) : null),
    setItem: (k, v) => { store.set(String(k), String(v)); },
    removeItem: (k) => { store.delete(String(k)); },
    clear: () => store.clear(),
    key: (i) => [...store.keys()][i] ?? null,
    get length() { return store.size; },
  };
  Object.defineProperty(window, "localStorage", { configurable: true, value: localStorage });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: localStorage });
}

// Component tests mount into a real document; unmounting between tests is what
// keeps one test's toasts, dialogs and event listeners out of the next one's
// queries. Imported lazily so the node-environment files pay nothing for it.
if (typeof document !== "undefined") {
  const { cleanup } = await import("@testing-library/react");
  afterEach(() => cleanup());
}
