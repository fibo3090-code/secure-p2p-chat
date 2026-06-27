// Minimal global toast store (pub/sub) so any component can surface feedback.
let seq = 0;
let toasts = [];
const subs = new Set();

function emit() { subs.forEach((fn) => fn(toasts)); }

export function toast(message, level = "info", ms = 4000) {
  const t = { id: ++seq, message: String(message), level };
  toasts = [...toasts, t];
  emit();
  if (ms) setTimeout(() => dismiss(t.id), ms);
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
