// The mockup references icons by short camelCase names (lucide icon set).
// Icons are imported **individually** so the bundler tree-shakes the rest of
// lucide-react — a namespace import (`import * as Lucide`) with a dynamic
// lookup ships the entire icon library (~1500 icons) and dominated the app
// bundle. Unknown names degrade to a dot so a typo never breaks the build.
import {
  ArrowDown, ArrowLeft, ArrowLeftRight, ArrowUp, Check, ChevronDown,
  ChevronRight, Circle, Clock, Copy, Download, EllipsisVertical, Eye, EyeOff,
  File, Fingerprint, Folder, Globe, Hash, Info, Key, Lock, LockOpen,
  MessageSquare, Monitor, Moon, Paperclip, Pencil, Plug, Plus, RefreshCw,
  Satellite, Search, Send, Server, Settings, Shield, ShieldCheck, Sun,
  Trash2, TriangleAlert, User, Users, X,
} from "lucide-react";

const MAP = {
  shieldCheck: ShieldCheck, shield: Shield, plus: Plus, x: X, check: Check,
  sun: Sun, moon: Moon, lock: Lock, unlock: LockOpen,
  message: MessageSquare, users: Users, user: User, server: Server,
  settings: Settings, search: Search, send: Send, paperclip: Paperclip,
  file: File, download: Download, alert: TriangleAlert, fingerprint: Fingerprint,
  key: Key, eye: Eye, eyeOff: EyeOff, arrowLeft: ArrowLeft, arrowUp: ArrowUp,
  arrowDown: ArrowDown, chevronRight: ChevronRight, chevronDown: ChevronDown,
  copy: Copy, refresh: RefreshCw, clock: Clock, globe: Globe, trash: Trash2,
  swap: ArrowLeftRight, more: EllipsisVertical, edit: Pencil, info: Info,
  plug: Plug, satellite: Satellite, hash: Hash, monitor: Monitor,
  folder: Folder,
};

export function Icon({ name, size = 18, ...rest }) {
  const Cmp = MAP[name] || Circle;
  return <Cmp size={size} strokeWidth={1.9} {...rest} />;
}
