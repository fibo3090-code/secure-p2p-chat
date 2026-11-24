// Interface text size. Every size in the stylesheet is in px, so scaling is
// applied as a zoom on the document — the same thing Ctrl +/− does in the
// window (zoomHotkeysEnabled in tauri.conf.json), kept as a setting so it
// survives a restart. Stored through `localStore` like the theme.
import { read, write } from "./localStore.js";

export const TEXT_SIZES = [
  { id: "small", label: "Smaller", scale: 0.92 },
  { id: "default", label: "Default", scale: 1 },
  { id: "large", label: "Larger", scale: 1.12 },
  { id: "xlarge", label: "Largest", scale: 1.25 },
];

const KEY = "p2pem.textSize";
const isKnown = (id) => TEXT_SIZES.some((t) => t.id === id);

export function loadTextSize() {
  return read(KEY, isKnown) ?? "default";
}

export function saveTextSize(id) {
  if (isKnown(id)) write(KEY, id);
}

export function applyTextSize(id) {
  const t = TEXT_SIZES.find((x) => x.id === id) || TEXT_SIZES[1];
  document.documentElement.style.zoom = t.scale === 1 ? "" : String(t.scale);
}
