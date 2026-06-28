import { colorGrid } from "../lib/colorgrid.js";

// Visual fingerprint — an n×n grid of deterministic colors both peers can
// eyeball-compare out of band.
export function SafetyGrid({ fingerprint, n = 8, cell = 26 }) {
  const cells = colorGrid(fingerprint, n);
  return (
    <div className="verify-grid-wrap" style={{ gridTemplateColumns: `repeat(${n}, 1fr)` }}>
      {cells.map((c, i) => (
        <span key={i} className="verify-cell" style={{ background: c, width: cell, height: cell }} />
      ))}
    </div>
  );
}
