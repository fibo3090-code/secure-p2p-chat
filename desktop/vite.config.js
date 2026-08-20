import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { cspMetaTag } from "./src/lib/csp.js";

// Inject the Content Security Policy into the document head at build time.
//
// It is injected rather than written into index.html by hand because dev and
// production need different `connect-src` values (Vite's HMR websocket), and a
// single hand-written tag would have to be permissive enough for dev — which is
// exactly the localhost exception that must not ship. See src/lib/csp.js.
function cspPlugin() {
  return {
    name: "p2pem-csp",
    transformIndexHtml(html, ctx) {
      const dev = !!ctx.server;
      return html.replace("<head>", `<head>\n    ${cspMetaTag({ dev })}`);
    },
  };
}

// Vite config tuned for Tauri: fixed dev port, no clearing the cargo output,
// build into ../dist (which tauri.conf.json embeds as frontendDist).
export default defineConfig({
  plugins: [react(), cspPlugin()],
  clearScreen: false,
  server: {
    // Bind an explicit address rather than letting Vite choose.
    //
    // Left unset, Vite can end up listening on `[::1]` only, while
    // `tauri.conf.json` points the webview at `localhost:5173` — and whether
    // that resolves to `::1` or `127.0.0.1` first is a property of the machine,
    // not of this project. When the two disagree the webview gets
    // connection-refused and shows a blank window with nothing to explain it,
    // which is indistinguishable from every other cause of a blank window.
    //
    // Pinning both sides to `127.0.0.1` takes name resolution out of the loop.
    // Keep this in step with `devUrl` in `tauri.conf.json`.
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
    // Two environments in one suite: the pure modules (password policy, safety
    // grid, tokens) need nothing, and the component tests need a DOM. Node is
    // the default so the cheap tests stay cheap; a component file opts in with
    // `// @vitest-environment jsdom` at the top.
    environment: "node",
    // jsdom only exposes `localStorage` for a document with a real origin; with
    // the default opaque one it is simply absent, and the storage-backed modules
    // (theme, Communities read marks) would be tested against nothing.
    environmentOptions: { jsdom: { url: "http://localhost/" } },
    setupFiles: ["./src/test/setup.js"],
    globals: true,
  },
});
