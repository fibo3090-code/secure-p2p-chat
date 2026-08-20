// The page's Content Security Policy, in one place.
//
// `tauri.conf.json` already sets a CSP header for the packaged webview, but the
// header was the *only* copy: `desktop/dist/index.html` carried no policy of its
// own, so the document was unprotected everywhere the header does not reach —
// `npm run dev` in a browser, `vite preview`, anyone opening the built page
// directly, and any future host that serves `dist/` without re-adding the
// header. A CSP that exists only in the shell's configuration is a CSP that a
// packaging change can silently drop.
//
// So the policy is also emitted as a `<meta http-equiv>` into the built HTML.
// The two must agree: when both a header and a meta tag are present the browser
// enforces the *intersection*, and a meta tag stricter than the header would
// break IPC in the packaged app while looking fine in dev. `csp.test.js` asserts
// they match, so editing one and forgetting the other fails the suite.
//
// Why each directive is what it is:
//
//   default-src 'self'  — nothing loads from anywhere but the bundle. This app
//                         deliberately makes no third-party requests at all: a
//                         font or analytics fetch on startup would leak that the
//                         user is running a secure messenger, and to whom.
//   img-src  'self' data: — `data:` is required: file previews cross the bridge
//                         base64-encoded rather than as paths (`file_preview`).
//   style-src 'unsafe-inline' — React sets inline `style` attributes for
//                         progress bars and avatar colours. Style injection is
//                         not script execution; dropping it would mean a runtime
//                         style pipeline for no security gain here.
//   script-src 'self'   — no `unsafe-inline`, no `unsafe-eval`. This is the
//                         directive that matters: it is what makes an injected
//                         `<script>` or a `javascript:` URL inert.
//   connect-src         — the Tauri IPC endpoints and nothing else.
//   object-src 'none'   — no plugins, ever.
//   base-uri 'self'     — a stray `<base>` cannot re-point every relative URL.
//   form-action 'none'  — nothing in this app submits a form anywhere.
//   frame-ancestors 'none' — the page is a desktop window, never framed. Header
//                         only: a meta tag cannot carry this directive.

/// Directives shared by every build. Order is kept stable so the string
/// comparison against `tauri.conf.json` is a plain equality check.
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

/// Directives a `<meta>` tag cannot carry. `frame-ancestors` is ignored — with a
/// console warning — when delivered that way, so it is emitted only in the
/// header the Tauri shell sets.
const HEADER_ONLY = [["frame-ancestors", ["'none'"]]];

/// Extra origins the Vite dev server needs, and only it: the HMR client opens a
/// websocket back to the dev server and fetches modules over http. These are
/// added *only* when building for development, so the shipped policy never
/// carries a localhost exception.
/// Both spellings of loopback: `vite.config.js` and `tauri.conf.json` pin
/// `127.0.0.1`, but a browser opened by hand will use `localhost`, and a policy
/// naming only one of them blocks HMR in whichever case it missed.
const DEV_CONNECT = [
  "ws://127.0.0.1:5173",
  "http://127.0.0.1:5173",
  "ws://localhost:5173",
  "http://localhost:5173",
];

/// Dev-only `script-src` additions.
///
/// `@vitejs/plugin-react` injects its React Refresh preamble as an **inline**
/// `<script type="module">`, and Vite serves its HMR client the same way. A bare
/// `script-src 'self'` blocks both: the preamble never runs, the entry module
/// throws, and the window comes up **blank** — with nothing on screen to explain
/// it, because the thing that would have rendered an error is what failed to
/// load. That is precisely what happened when the meta tag was first added, and
/// it only showed up in `tauri dev`, since Tauri's CSP *header* cannot apply to
/// a page the Vite server delivers.
///
/// This is not a relaxation of the shipped policy. Production keeps
/// `script-src 'self'` with no inline allowance — the directive that makes an
/// injected `<script>` inert — and `the shipped policy allows no inline script`
/// asserts it.
const DEV_SCRIPT = ["'unsafe-inline'"];

/// The policy string.
///
/// `dev: true` widens `connect-src` for Vite's HMR socket and `script-src` for
/// its inline preamble. `forHeader: true`
/// adds the directives only a real header can carry — that is the form
/// `tauri.conf.json` must hold.
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
