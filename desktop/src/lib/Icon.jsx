import * as Lucide from "lucide-react";

// The mockup references icons by short camelCase names (lucide icon set).
// Map them to lucide-react components; unknown names degrade to a dot so a
// typo never breaks the build.
const MAP = {
  shieldCheck: "ShieldCheck", shield: "Shield", plus: "Plus", x: "X", check: "Check",
  sun: "Sun", moon: "Moon", lock: "Lock", unlock: "LockOpen",
  message: "MessageSquare", users: "Users", user: "User", server: "Server",
  settings: "Settings", search: "Search", send: "Send", paperclip: "Paperclip",
  file: "File", download: "Download", alert: "TriangleAlert", fingerprint: "Fingerprint",
  key: "Key", eye: "Eye", eyeOff: "EyeOff", arrowLeft: "ArrowLeft", arrowUp: "ArrowUp",
  arrowDown: "ArrowDown", chevronRight: "ChevronRight", chevronDown: "ChevronDown",
  copy: "Copy", refresh: "RefreshCw", clock: "Clock", globe: "Globe", trash: "Trash2",
  swap: "ArrowLeftRight", more: "EllipsisVertical", edit: "Pencil", info: "Info",
  plug: "Plug", satellite: "Satellite", hash: "Hash", monitor: "Monitor",
  folder: "Folder",
};

export function Icon({ name, size = 18, ...rest }) {
  const Cmp = Lucide[MAP[name]] || Lucide.Circle;
  return <Cmp size={size} strokeWidth={1.9} {...rest} />;
}
