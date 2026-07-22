import importMetaUrlPlugin from "@codingame/esbuild-import-meta-url-plugin";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import path from "node:path";
import { defineConfig, type Plugin } from "vite";
import { resolveDevPort } from "../../scripts/task-port.mjs";
import pkg from "./package.json" with { type: "json" };

// @vscode/diff reads its .wasm off disk from a Node-only branch of `initWasm()`
// that a WebView never takes; left alone, every chunk pulling it in warns that
// `node:fs/promises` was externalized. Point that one dep at a shim that throws
// instead — deliberately scoped to it rather than aliased globally, so the next
// dep to reach for a Node builtin still says so at build time instead of
// silently resolving to a stub that only fails once something calls it.
function vscodeDiffNodeShim(): Plugin {
  const shim = path.resolve(__dirname, "./src/shims/node-fs-promises.ts");
  return {
    name: "tt:vscode-diff-node-shim",
    enforce: "pre",
    resolveId(source, importer) {
      if (source !== "node:fs/promises" || !importer?.includes("@vscode/diff")) return null;
      return shim;
    },
  };
}

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
//
// Resolved only when the dev *server* is actually going to bind it: a
// `vite build` never listens on anything, and failing it (as a top-level
// resolve did) broke every checkout without a rendered `.env` — CI first.
const repoRoot = path.resolve(__dirname, "../..");

function requireDevPort(): number {
  const port = Number(process.env.TT_DEV_PORT) || resolveDevPort(repoRoot).unwrapOr(undefined);
  if (!port) {
    throw new Error(
      "no TT_DEV_PORT for this checkout — run `tt task env <name>` to claim ports, " +
        "or pin TT_DEV_PORT in .env.local",
    );
  }
  return port;
}

// https://vitejs.dev/config/
export default defineConfig(({ command }) => ({
  plugins: [react(), tailwindcss(), vscodeDiffNodeShim()],
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
  // Worker builds are their own rollup pass and do NOT inherit `plugins`, so
  // the @vscode/diff shim has to be registered a second time here — the worker
  // graph pulls that dep in too, and without this it warns three more times.
  worker: {
    format: "es",
    plugins: () => [vscodeDiffNodeShim()],
  },
  // The main chunk is ~2.4 MB minified and that is accepted, not an
  // oversight: the monaco-vscode stack must stay one module graph (see the
  // dedupe note above — splitting it breaks grammar/theme registration),
  // screens are static imports by design (apps/client/CLAUDE.md's motion
  // note), and a Tauri webview loads assets from local disk, so the 500 kB
  // default — a network-delivery heuristic — doesn't apply. The limit is
  // raised with headroom rather than removed: growth past ~3 MB should
  // resurface the warning and prompt a fresh look.
  build: {
    chunkSizeWarningLimit: 3000,
  },
  // Prevent Vite from obscuring Rust errors
  clearScreen: false,
  server:
    command === "serve"
      ? {
          port: requireDevPort(),
          strictPort: true,
        }
      : undefined,
  // Env variables starting with these prefixes are exposed to the client
  envPrefix: ["VITE_", "TAURI_"],
}));
