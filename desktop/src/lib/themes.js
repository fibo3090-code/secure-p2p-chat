// Theme registry + persistence. data-theme is applied to <html>; the swatch
// is just for the picker UI.
import { read, write } from "./localStore.js";

export const THEMES = [
  { id: "dark", label: "Slate", swatch: "#3e8dd2" },
  { id: "midnight", label: "Midnight", swatch: "#8b7bff" },
  { id: "forest", label: "Forest", swatch: "#34d399" },
  { id: "rose", label: "Rosé", swatch: "#fb6f92" },
  { id: "light", label: "Daylight", swatch: "#3e8dd2" },
];

const KEY = "p2pem.theme";

const isKnownTheme = (id) => THEMES.some((t) => t.id === id);

// Stored through `localStore`, which checksums the entry and refuses one that
// has been damaged or copied in from another key. The theme is cosmetic, so the
// win is not confidentiality — it is that a corrupt profile falls back to the
// default instead of applying a half-written value, and that the app has one
// place where persisted UI state is validated rather than two hand-rolled ones.
export function loadTheme() {
  // `legacy`: theme used to be stored as the bare id. Accept that once and
  // rewrite it in envelope form so existing installs keep their choice.
  return read(KEY, isKnownTheme, (raw) => raw) ?? "dark";
}

export function saveTheme(id) {
  if (!isKnownTheme(id)) return;
  write(KEY, id);
}
