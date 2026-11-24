import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { cspMetaTag } from "./src/lib/csp.js";

// Inject the CSP into the document head. Dev needs a different `connect-src`
// (Vite's HMR websocket), and one hand-written tag would ship that localhost
// exception. See src/lib/csp.js.
function cspPlugin() {
  return {
    name: "p2pem-csp",
    transformIndexHtml(html, ctx) {
      const dev = !!ctx.server;
      return html.replace("<head>", `<head>\n    ${cspMetaTag({ dev })}`);
    },
  };
}

// Tuned for Tauri: fixed dev port, cargo output not cleared, build into `dist/`
// (tauri.conf.json's frontendDist).
export default defineConfig({
  plugins: [react(), cspPlugin()],
  clearScreen: false,
  server: {
    // An explicit address: left unset, Vite may listen on `[::1]` only while the
    // webview resolves `localhost` to `127.0.0.1` — a blank window with nothing to
    // explain it. Keep in step with `devUrl` in `tauri.conf.json`.
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "esnext",
  },
  test: {
    // Vitest owns `src/**/*.test.*`, Playwright owns `smoke/`: the default glob
    // would also sweep up the `*.spec.js` smoke tests and fail on their API.
    include: ["src/**/*.test.{js,jsx}"],
    // Node by default so pure-module tests stay cheap; component files opt in
    // with `// @vitest-environment jsdom`.
    environment: "node",
    // jsdom only exposes `localStorage` for a real origin; without it the
    // storage-backed modules would be tested against nothing.
    environmentOptions: { jsdom: { url: "http://localhost/" } },
    setupFiles: ["./src/test/setup.js"],
    globals: true,
  },
});
