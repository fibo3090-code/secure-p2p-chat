// Password policy for the set-password screen. The floor comes from the bridge
// (`auth.min_password_len`) and `Identity::encrypt` enforces it; this only
// explains it, so the UI never coaches one length while accepting another.

// Used only if the bridge did not report a floor (a mock, or an older backend).
export const FALLBACK_MIN_PASSWORD = 12;

const floorOf = (min) => (Number.isFinite(min) && min > 0 ? min : FALLBACK_MIN_PASSWORD);

/**
 * Score a candidate password against the enforced floor. Below it there is no
 * "weak but allowed" tier: the label says how many characters are missing.
 *
 * @returns {{score: number, label: string, ok: boolean}} score is 0–4.
 */
export function pwStrength(pw, min = FALLBACK_MIN_PASSWORD) {
  const floor = floorOf(min);
  // Code points, like the Rust side's `chars()`.
  const len = [...(pw || "")].length;
  if (!len) return { score: 0, label: `Use at least ${floor} characters`, ok: false };
  if (len < floor) {
    const missing = floor - len;
    return {
      score: 0,
      label: `${missing} more character${missing === 1 ? "" : "s"} needed`,
      ok: false,
    };
  }
  let s = 1;
  if (len >= floor + 4) s++;
  if (/[A-Z]/.test(pw) && /[0-9]/.test(pw)) s++;
  if (/[^A-Za-z0-9]/.test(pw)) s++;
  const labels = ["", "Acceptable", "Good", "Strong", "Excellent"];
  const score = Math.min(s, 4);
  return { score, label: labels[score], ok: true };
}

/** Why a set-password form cannot be submitted, or "" when it can. */
export function passwordFormError(pw, confirm, min = FALLBACK_MIN_PASSWORD) {
  const floor = floorOf(min);
  if (!pwStrength(pw, floor).ok) return `Password must be at least ${floor} characters.`;
  if (!confirm) return "Confirm your password.";
  if (pw !== confirm) return "The two passwords don't match.";
  return "";
}
