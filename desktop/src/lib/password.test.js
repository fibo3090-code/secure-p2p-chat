import { describe, it, expect } from "vitest";
import { pwStrength, passwordFormError, FALLBACK_MIN_PASSWORD } from "./password.js";

describe("pwStrength", () => {
  it("refuses anything under the floor instead of grading it", () => {
    // The screen used to coach "12+" while accepting four characters. Below the
    // floor there is no passing score at all.
    for (const pw of ["", "a", "abcd", "a".repeat(FALLBACK_MIN_PASSWORD - 1)]) {
      expect(pwStrength(pw, FALLBACK_MIN_PASSWORD).ok).toBe(false);
    }
  });

  it("says how many characters are still needed", () => {
    expect(pwStrength("abcdefghij", 12).label).toBe("2 more characters needed");
    expect(pwStrength("abcdefghijk", 12).label).toBe("1 more character needed");
  });

  it("accepts exactly the floor", () => {
    const r = pwStrength("a".repeat(12), 12);
    expect(r.ok).toBe(true);
    expect(r.score).toBeGreaterThan(0);
  });

  it("rewards length and character variety", () => {
    const plain = pwStrength("aaaaaaaaaaaa", 12);
    const mixed = pwStrength("Abcdefgh1234!x", 12); // long enough, mixed case+digit+symbol
    expect(mixed.score).toBeGreaterThan(plain.score);
    expect(mixed.label).toBe("Strong");
    // Top score also needs real length, not just variety.
    expect(pwStrength("Abcdefghij1234!xyz", 12).label).toBe("Excellent");
  });

  it("counts code points, matching the Rust side's chars()", () => {
    // 12 astral-plane code points are 24 UTF-16 units; a `.length` check would
    // wrongly pass this at half the intended strength.
    expect(pwStrength("🔒".repeat(12), 12).ok).toBe(true);
    expect(pwStrength("🔒".repeat(11), 12).ok).toBe(false);
  });

  it("falls back to a safe floor when the backend reports none", () => {
    expect(pwStrength("a".repeat(FALLBACK_MIN_PASSWORD - 1), undefined).ok).toBe(false);
    expect(pwStrength("a".repeat(FALLBACK_MIN_PASSWORD), null).ok).toBe(true);
  });
});

describe("passwordFormError", () => {
  it("names the specific problem so the button is never a silent no-op", () => {
    expect(passwordFormError("short", "short", 12)).toMatch(/at least 12/);
    expect(passwordFormError("a".repeat(12), "", 12)).toMatch(/Confirm/);
    expect(passwordFormError("a".repeat(12), "b".repeat(12), 12)).toMatch(/don't match/);
  });

  it("returns empty when the form is valid", () => {
    expect(passwordFormError("correct-horse-battery", "correct-horse-battery", 12)).toBe("");
  });
});
