// Theme registry + persistence. data-theme is applied to <html>; the swatch
// is just for the picker UI.
import { read, write } from "./localStore.js";

export const THEMES = [
  { id: "dark", label: "Slate", swatch: "#4a68ec" },
  { id: "midnight", label: "Midnight", swatch: "#8b7bff" },
  { id: "forest", label: "Forest", swatch: "#34d399" },
  { id: "rose", label: "Rosé", swatch: "#fb6f92" },
  { id: "light", label: "Daylight", swatch: "#3f5fd9" },
];

const KEY = "p2pem.theme";

const isKnownTheme = (id) => THEMES.some((t) => t.id === id);

// A damaged entry falls back to the default (see `localStore`).
export function loadTheme() {
  // `legacy`: the bare id older installs stored, migrated once.
  return read(KEY, isKnownTheme, (raw) => raw) ?? "dark";
}

export function saveTheme(id) {
  if (!isKnownTheme(id)) return;
  write(KEY, id);
}
