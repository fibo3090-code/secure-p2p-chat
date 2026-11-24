// The page's Content Security Policy, in one place. `tauri.conf.json` sets it
// as a header for the packaged webview, and it is also emitted as a `<meta>`
// into the built HTML so the page is protected where the header does not reach
// (dev, `vite preview`, opening `dist/` directly). The browser enforces the
// intersection, so the two must agree; `csp.test.js` asserts it.
//
//   default-src 'self'    — no third-party requests: one would leak that the
//                           user runs a secure messenger.
//   img-src data:         — file previews cross the bridge base64-encoded.
//   style-src 'unsafe-inline' — React's inline `style` attributes; style
//                           injection is not script execution.
//   script-src 'self'     — no inline/eval: an injected `<script>` or
//                           `javascript:` URL stays inert. The one that matters.
//   connect-src           — the Tauri IPC endpoints and nothing else.
//   object-src 'none', base-uri 'self', form-action 'none'.
//   frame-ancestors 'none' — header only: a meta tag cannot carry it.

/// Directives shared by every build, in a stable order so the comparison with
/// `tauri.conf.json` is plain equality.
const BASE = [
  ["default-src", ["'self'"]],
  ["img-src", ["'self'", "data:"]],
  ["font-src", ["'self'"]],
  ["style-src", ["'self'", "'unsafe-inline'"]],
  ["script-src", ["'self'"]],
  ["connect-src", ["'self'", "ipc:", "http://ipc.localhost"]],
  ["object-src", ["'none'"]],
  ["base-uri", ["'self'"]],
  ["form-action", ["'none'"]],
];

/// Directives a `<meta>` tag cannot carry (`frame-ancestors` is ignored there).
const HEADER_ONLY = [["frame-ancestors", ["'none'"]]];

/// Vite dev server origins for HMR, added only in development, in both
/// loopback spellings (config pins `127.0.0.1`; a hand-opened browser uses
/// `localhost`).
const DEV_CONNECT = [
  "ws://127.0.0.1:5173",
  "http://127.0.0.1:5173",
  "ws://localhost:5173",
  "http://localhost:5173",
];

/// Dev-only `script-src` addition: Vite and the React Refresh preamble inject
/// an inline `<script type="module">`, and blocking it leaves a **blank**
/// window (which happened). Production keeps `script-src 'self'`, asserted by
/// `the shipped policy allows no inline script`.
const DEV_SCRIPT = ["'unsafe-inline'"];

/// The policy string. `dev` widens `connect-src`/`script-src` for Vite;
/// `forHeader` adds the header-only directives (the `tauri.conf.json` form).
export function cspPolicy({ dev = false, forHeader = false } = {}) {
  const directives = forHeader ? [...BASE, ...HEADER_ONLY] : BASE;
  return directives
    .map(([name, values]) => {
      let all = values;
      if (dev && name === "connect-src") all = [...all, ...DEV_CONNECT];
      if (dev && name === "script-src") all = [...all, ...DEV_SCRIPT];
      return `${name} ${all.join(" ")}`;
    })
    .join("; ");
}

/// The `<meta>` tag to inject into the document head.
export function cspMetaTag(opts) {
  return `<meta http-equiv="Content-Security-Policy" content="${cspPolicy(opts)}">`;
}
