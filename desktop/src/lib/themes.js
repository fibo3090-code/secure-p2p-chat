// Theme registry + persistence. data-theme is applied to <html>; the swatch
// is just for the picker UI.
export const THEMES = [
  { id: "dark", label: "Slate", swatch: "#3e8dd2" },
  { id: "midnight", label: "Midnight", swatch: "#8b7bff" },
  { id: "forest", label: "Forest", swatch: "#34d399" },
  { id: "rose", label: "Rosé", swatch: "#fb6f92" },
  { id: "light", label: "Daylight", swatch: "#3e8dd2" },
];

const KEY = "p2pem.theme";

export function loadTheme() {
  const saved = localStorage.getItem(KEY);
  return THEMES.some((t) => t.id === saved) ? saved : "dark";
}

export function saveTheme(id) {
  try { localStorage.setItem(KEY, id); } catch { /* ignore */ }
}
