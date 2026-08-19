// Turning strings from the user or the OS into values the app can trust.
// Kept dependency-free so it can be unit-tested without React or the bridge.

/**
 * Strict port parse. Returns `null` when the input is not a port, so callers can
 * say what is wrong instead of guessing.
 *
 * `parseInt` alone is too forgiving here: it accepts "80abc" (→ 80) and returns
 * NaN for "", both of which used to collapse into a silent fallback to 12345.
 * The app then dialled a port the user never typed and reported a bare
 * "connection refused", with the address on screen still showing their version.
 */
export function parsePort(raw) {
  const s = String(raw ?? "").trim();
  if (!/^\d{1,5}$/.test(s)) return null;
  const n = Number(s);
  return n >= 1 && n <= 65535 ? n : null;
}

/**
 * The name of the folder containing `p`, for both Windows and POSIX paths.
 *
 * File cards used to claim "saved to Downloads" for every received file, which
 * is wrong for anyone who chose a different download folder — and that card is
 * the only place the app ever says where a file went. Deriving the folder from
 * the recorded path is also right for files received *before* the setting was
 * changed, which reading the current setting would not be.
 *
 * Returns "" when there is no containing folder to name (a bare filename, an
 * empty value, or a filesystem root).
 */
export function folderOf(p) {
  const s = String(p ?? "");
  const cut = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
  if (cut <= 0) return "";
  const dir = s.slice(0, cut);
  const parent = Math.max(dir.lastIndexOf("/"), dir.lastIndexOf("\\"));
  return parent >= 0 ? dir.slice(parent + 1) : dir;
}
