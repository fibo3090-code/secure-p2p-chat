// Deterministic color "safety grid" from a fingerprint hex string. Both peers
// derive the identical grid from the same fingerprint, so comparing the pattern
// out of band is an easy visual TOFU check. n×n cells (default 8×8 = 64).
export function colorGrid(fp, n = 8) {
  // Fail closed: a missing/invalid fingerprint yields no grid rather than a
  // fabricated pattern, so the verify flow can't present a bogus comparison
  // target for TOFU.
  const s = String(fp ?? "").replace(/[^0-9a-f]/gi, "");
  if (!s) return [];
  const cells = [];
  for (let i = 0; i < n * n; i++) {
    let h = (2166136261 ^ i) >>> 0; // FNV-ish, salted by index
    for (let k = 0; k < s.length; k++) {
      h = ((h ^ s.charCodeAt((k + i) % s.length)) * 16777619) >>> 0;
    }
    const hue = h % 360;
    const sat = 45 + ((h >>> 9) % 30);
    const lig = 46 + ((h >>> 17) % 16);
    cells.push(`hsl(${hue} ${sat}% ${lig}%)`);
  }
  return cells;
}
