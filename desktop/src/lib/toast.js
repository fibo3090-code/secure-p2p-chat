// Minimal global toast store (pub/sub) so any component can surface feedback.
let seq = 0;
let toasts = [];
const subs = new Set();

// Confirmations expire; errors stay until dismissed (four seconds is too short
// to read one, or for a screen reader to announce it).
export const DEFAULT_TTL_MS = 4000;

// Cap held toasts so a flapping connection cannot bury the screen; oldest go first.
export const MAX_TOASTS = 6;

function emit() { subs.forEach((fn) => fn(toasts)); }

/** Show a toast. `ms` overrides the default lifetime; 0 means manual dismiss. */
export function toast(message, level = "info", ms) {
  const t = { id: ++seq, message: String(message), level };
  const ttl = ms === undefined ? (level === "error" ? 0 : DEFAULT_TTL_MS) : ms;
  toasts = [...toasts, t].slice(-MAX_TOASTS);
  emit();
  if (ttl) setTimeout(() => dismiss(t.id), ttl);
  return t.id;
}

export function dismiss(id) {
  toasts = toasts.filter((t) => t.id !== id);
  emit();
}

export function subscribe(fn) {
  subs.add(fn);
  fn(toasts);
  return () => subs.delete(fn);
}
