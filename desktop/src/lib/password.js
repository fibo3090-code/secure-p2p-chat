// Password policy for the set-password screen.
//
// The floor is NOT decided here: the bridge reports it (`auth.min_password_len`,
// mirroring `messenger_core::MIN_PASSWORD_LEN`), and `Identity::encrypt` enforces
// it. This module only turns that number into something a user can act on, so
// the UI can never coach one length while accepting another.

// Used only if the bridge did not report a floor (a mock, or an older backend).
export const FALLBACK_MIN_PASSWORD = 12;

/**
 * Score a candidate password against the enforced floor.
 *
 * Below the floor there is deliberately no "weak but allowed" tier — the meter
 * says how many characters are still needed and `ok` stays false, so the submit
 * button can be disabled for a reason the user can see.
 *
 * @returns {{score: number, label: string, ok: boolean}} score is 0–4.
 */
export function pwStrength(pw, min = FALLBACK_MIN_PASSWORD) {
  const floor = Number.isFinite(min) && min > 0 ? min : FALLBACK_MIN_PASSWORD;
  // Count code points, not UTF-16 units, so an emoji-heavy passphrase is not
  // over-counted relative to the Rust side (which counts `chars()`).
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

/**
 * The reason a set-password form cannot be submitted, or "" when it can.
 * Returning the reason (rather than just a boolean) is what lets the screen
 * explain itself instead of making the button silently do nothing.
 */
export function passwordFormError(pw, confirm, min = FALLBACK_MIN_PASSWORD) {
  const floor = Number.isFinite(min) && min > 0 ? min : FALLBACK_MIN_PASSWORD;
  if (!pwStrength(pw, floor).ok) return `Password must be at least ${floor} characters.`;
  if (!confirm) return "Confirm your password.";
  if (pw !== confirm) return "The two passwords don't match.";
  return "";
}
