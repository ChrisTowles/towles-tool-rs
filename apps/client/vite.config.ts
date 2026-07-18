import importMetaUrlPlugin from "@codingame/esbuild-import-meta-url-plugin";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig } from "vite";
import { resolveDevPort } from "../../scripts/slot-port.mjs";
import pkg from "./package.json" with { type: "json" };

// Every @codingame/monaco-vscode-* package must be pre-bundled together (and
// deduped) so they share one module instance — otherwise, in dev, the
// default-extension packages register grammars/themes into a different copy
// of the api than the one `initialize` starts, and nothing highlights.
const monacoVscodeDeps = Object.keys(pkg.dependencies).filter((d) =>
  d.startsWith("@codingame/monaco-vscode-"),
);

// `dev-port.mjs` normally pins TT_DEV_PORT before launching us. Run directly
// (bare-vite mock dev), resolve the same per-checkout claim from the repo
// root's rendered `.env`/`.env.local`.
//
// There is deliberately no fallback port. Any value picked outside the claim
// system is drawn from the same 1420-1619 pool the claims come from, so it
// collides with whichever sibling checkout claimed it — 1420 in particular is
// the pool's first port, and therefore almost always already held. Failing
// here with the fix is better than binding a port that isn't ours.
const repoRoot = path.resolve(__dirname, "../..");
const devPort =
  Number(process.env.TT_DEV_PORT) ||
  resolveDevPort(repoRoot).unwrapOr(undefined) ||
  fatalNoDevPort();

function fatalNoDevPort(): never {
  throw new Error(
    "no TT_DEV_PORT for this checkout — run `tt slot env <name>` to claim ports, " +
      "or pin TT_DEV_PORT in .env.local",
  );
}

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
    dedupe: ["monaco-editor", "vscode", ...monacoVscodeDeps],
  },
  // monaco-vscode-api relies on `new URL(..., import.meta.url)` inside deps
  // (broken by Vite's dep pre-bundling without this plugin) and ships some
  // CommonJS-only transitive deps that must be pre-bundled to load in workers.
  optimizeDeps: {
    include: [
      ...monacoVscodeDeps,
      "@codingame/monaco-vscode-api/extensions",
      "@codingame/monaco-vscode-api/monaco",
      "monaco-editor",
      "monaco-languageclient",
      "vscode-languageclient/browser",
      "vscode-jsonrpc",
      "vscode-textmate",
      "vscode-oniguruma",
    ],
    // importMetaUrlPlugin can't resolve @vscode/diff's `worker.js?esm` URL —
    // serve it unbundled instead of pre-optimizing it.
    exclude: ["@vscode/diff"],
    esbuildOptions: {
      plugins: [importMetaUrlPlugin],
    },
  },
  // The textmate tokenization worker code-splits, which rollup only supports
  // with ES-module workers (module workers are fine in WebKitGTK/WebView2).
  worker: {
    format: "es",
  },
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  server: {
    port: devPort,
    strictPort: true,
  },
  // Env variables starting with these prefixes are exposed to the client
  envPrefix: ["VITE_", "TAURI_"],
});
