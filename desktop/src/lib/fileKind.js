// What a file *is*, for display. The decision to refuse opening a peer's
// program lives in the bridge (`execution_risk`, src-tauri/src/commands/chats.rs)
// and stays there: this module only lets the card say so *before* the click,
// so a `holiday.jpg.exe` stops looking exactly like `roadmap.pdf`.
//
// The list of extensions the OS would execute is the bridge's own
// (`RISKY_EXTENSIONS` in bridge.js, which `bridge.test.js` compares with
// chats.rs). This module used to keep a second copy, which fell behind the
// first as soon as the list grew: a card calling a `.dmg` a "DMG file" while
// the click asked "this is a program — open anyway?".
import { RISKY_EXTENSIONS } from "./bridge.js";

export { RISKY_EXTENSIONS };

const KINDS = [
  [/^(png|jpe?g|gif|webp|bmp|heic|svg|tiff?)$/, "Image", "fileImage"],
  [/^(pdf)$/, "PDF", "fileText"],
  [/^(docx?|odt|rtf|txt|md|pages)$/, "Document", "fileText"],
  [/^(xlsx?|ods|csv|numbers)$/, "Spreadsheet", "fileText"],
  [/^(pptx?|odp|key)$/, "Presentation", "fileText"],
  [/^(zip|7z|rar|tar|gz|tgz|bz2|xz)$/, "Archive", "file"],
  [/^(mp3|wav|flac|ogg|m4a|aac|opus)$/, "Audio", "file"],
  [/^(mp4|mov|mkv|webm|avi)$/, "Video", "file"],
];

/** Split a filename into its stem and *final* extension (the one the OS uses). */
export function splitName(name) {
  const n = String(name || "");
  const i = n.lastIndexOf(".");
  if (i <= 0 || i === n.length - 1) return { stem: n, ext: "" };
  return { stem: n.slice(0, i), ext: n.slice(i + 1) };
}

/**
 * `{ label, icon, risky, ext }` for a filename. `risky` is true when opening it
 * would hand it to the OS to run.
 */
export function fileKind(name) {
  const { ext } = splitName(name);
  const e = ext.toLowerCase();
  if (RISKY_EXTENSIONS.has(e)) return { label: "Program", icon: "fileWarn", risky: true, ext: e };
  for (const [re, label, icon] of KINDS) if (re.test(e)) return { label, icon, risky: false, ext: e };
  return { label: e ? e.toUpperCase() : "File", icon: "file", risky: false, ext: e };
}
