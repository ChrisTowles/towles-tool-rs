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

// `dev-port.mjs` normally pins TT_DEV_PORT before launching us. If vite is run
// directly, resolve the same per-checkout claim from the repo root's rendered
// `.env`/`.env.local`; a checkout with no claim at all gets 1420 (bare-vite
// mock dev only — every tt-managed checkout has a claim, so this never
// collides across slots in practice).
const devPort =
  Number(process.env.TT_DEV_PORT) || resolveDevPort(path.resolve(__dirname, "../..")) || 1420;

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
    dedupe: ["monaco-editor", ...monacoVscodeDeps],
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
