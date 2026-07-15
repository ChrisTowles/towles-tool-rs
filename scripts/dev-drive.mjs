#!/usr/bin/env node
// Launch the app as a PERSISTENT, automatable dev window.
//
// This is `npm run dev` (HMR, source watch, you use the window normally) plus the
// one thing plain dev deliberately omits: the app is built with the `wdio` cargo
// feature and launched with WebKit automation on, so `tauri-plugin-wdio-webdriver`
// runs its in-app W3C WebDriver server on `wdPort` for the whole lifetime of the
// window. `scripts/drive.mjs` connects to that server to drive the window while
// you watch — no WDIO, no spawn/kill. See e2e/README.md ("Live drive").
//
// Ports come from `.env.local` exactly like `npm run dev` (deterministic per slot,
// NOT a free-port scan) so `drive.mjs` can find the automation server without
// being told: wdPort = TT_DEV_PORT + 3000 (override with TT_E2E_WEBDRIVER_PORT).
import { fileURLToPath } from "node:url";
import path from "node:path";
import { resolveDevPort, resolveWebdriverPort, spawnTauriDev } from "./slot-port.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const devPort = resolveDevPort(repoRoot);
if (!devPort) {
  console.error(
    `[dev-drive] TT_DEV_PORT=${process.env.TT_DEV_PORT} is not a valid port (1-65535)`,
  );
  process.exit(1);
}
const wdPort = resolveWebdriverPort(devPort);

console.log(`[dev-drive] dev server ${devPort} · automation server ${wdPort}`);
console.log(
  `[dev-drive] once the window is up: node scripts/drive.mjs status  (→ http://127.0.0.1:${wdPort}/status)`,
);

// `tauri dev` builds the app (with our feature) and runs beforeDevCommand (vite).
// devUrl is baked to the dev port so the WebView is a trusted Tauri origin (IPC
// invokes allowed); VITE_WDIO makes the frontend load @wdio/tauri-plugin.
spawnTauriDev(
  [
    "dev",
    "--features",
    "wdio",
    "--config",
    JSON.stringify({ build: { devUrl: `http://localhost:${devPort}` } }),
  ],
  {
    ...process.env,
    TT_DEV_PORT: String(devPort),
    VITE_WDIO: "1",
    TAURI_WEBVIEW_AUTOMATION: "true",
    TAURI_WEBDRIVER_PORT: String(wdPort),
  },
);
