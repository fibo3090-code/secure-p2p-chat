// What a file *is*, for display. The decision to refuse opening a peer's
// program lives in the bridge (`execution_risk`, src-tauri/src/commands/chats.rs)
// and stays there: this module only lets the card say so *before* the click,
// so a `holiday.jpg.exe` stops looking exactly like `roadmap.pdf`.
//
// `RISKY_EXTENSIONS` mirrors the bridge's list; `fileKind.test.js` reads the
// Rust source and fails if the two drift apart. A mismatch here could only ever
// under-warn on the card — the guard dialog still fires — but a card that says
// "document" about something the OS will execute is the exact confusion this
// exists to remove.
export const RISKY_EXTENSIONS = [
  "exe", "com", "scr", "pif", "bat", "cmd", "msi", "msp", "msc", "cpl", "hta",
  "vbs", "vbe", "jse", "wsf", "wsh", "ps1", "psm1", "reg", "lnk", "url", "scf",
  "inf", "chm", "application", "gadget",
  "jar", "js", "py", "pl", "rb", "php",
  "sh", "bash", "zsh", "command", "desktop", "run", "appimage", "app", "pkg", "deb", "rpm",
];

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
  if (RISKY_EXTENSIONS.includes(e)) return { label: "Program", icon: "fileWarn", risky: true, ext: e };
  for (const [re, label, icon] of KINDS) if (re.test(e)) return { label, icon, risky: false, ext: e };
  return { label: e ? e.toUpperCase() : "File", icon: "file", risky: false, ext: e };
}
