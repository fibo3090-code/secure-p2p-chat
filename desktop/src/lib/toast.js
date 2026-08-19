// Minimal global toast store (pub/sub) so any component can surface feedback.
let seq = 0;
let toasts = [];
const subs = new Set();

// Most toasts are confirmations that can scroll by. Errors are not: they are the
// only place the app reports "not delivered", "transfer interrupted" or a failed
// verification, and four seconds is not enough to read one — let alone for a
// screen reader to finish announcing it. Errors therefore stay until dismissed.
export const DEFAULT_TTL_MS = 4000;

// Hard cap on how many are held at once. Errors no longer expire on their own,
// so without this a flapping connection would grow the list without bound and
// bury the screen (and the live region) under its own history. Oldest go first.
export const MAX_TOASTS = 6;

function emit() { subs.forEach((fn) => fn(toasts)); }

/**
 * Show a toast. `ms` defaults to 4s for info/success and to "stay until
 * dismissed" for errors; pass an explicit number to override, or 0 to require a
 * manual dismiss.
 */
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
