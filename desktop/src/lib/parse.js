// Turning strings from the user or the OS into values the app can trust.
// Kept dependency-free so it can be unit-tested without React or the bridge.

/**
 * Strict port parse: `null` when the input is not a port. (`parseInt` accepts
 * "80abc", and its NaN used to fall back silently to a port nobody typed.)
 */
export function parsePort(raw) {
  const s = String(raw ?? "").trim();
  if (!/^\d{1,5}$/.test(s)) return null;
  const n = Number(s);
  return n >= 1 && n <= 65535 ? n : null;
}

/**
 * The name of the folder containing `p` (Windows or POSIX), derived from the
 * recorded path so it stays right after the download setting changes. "" when
 * there is none (bare filename, empty value, filesystem root).
 */
export function folderOf(p) {
  const s = String(p ?? "");
  const cut = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
  if (cut <= 0) return "";
  const dir = s.slice(0, cut);
  const parent = Math.max(dir.lastIndexOf("/"), dir.lastIndexOf("\\"));
  return parent >= 0 ? dir.slice(parent + 1) : dir;
}
