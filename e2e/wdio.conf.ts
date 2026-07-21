import path from "node:path";
import { requireDevPort, resolveWebdriverPort } from "../scripts/task-port.mjs";

// Ports are injected by scripts/e2e.mjs (resolved from the rendered
// `.env`/`.env.local`). The dev server serves the wdio-enabled frontend; the
// embedded WebDriver server runs inside the app on wdPort. A direct
// `npx wdio` run without TT_DEV_PORT resolves the same per-checkout claim
// (and fails with instructions if the checkout has none) — never a
// hardcoded 1420, which would collide across concurrent worktrees.
const repoRoot = process.cwd();
const devPort = requireDevPort(repoRoot, { tag: "wdio" });
const wdPort = resolveWebdriverPort(devPort);

// Debug binary built with `--features wdio`. Run from repo root, so resolve cwd.
const appBinary = path.resolve(process.cwd(), "target/debug/tt-app");

export const config: WebdriverIO.Config = {
  runner: "local",
  tsConfigPath: path.resolve(process.cwd(), "e2e/tsconfig.json"),

  specs: ["./specs/**/*.e2e.ts"],
  maxInstances: 1,

  // The app boots pointing at its baked devUrl (unused here); we navigate the
  // automatable WebView to the live dev server in `before`.
  baseUrl: `http://localhost:${devPort}`,

  capabilities: [
    {
      browserName: "tauri",
      // @ts-expect-error tauri capability shape isn't in the base WDIO types
      "tauri:options": { application: appBinary },
    },
  ],

  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath: appBinary,
        // Embedded W3C WebDriver server (tauri-plugin-wdio-webdriver) — the app
        // is launched with TAURI_WEBVIEW_AUTOMATION=true (set by e2e.mjs) so its
        // WebKitGTK WebView is automatable.
        driverProvider: "embedded",
        embeddedPort: wdPort,
        // GTK/WebKit GUI cold-starts in ~6-8s; give the readiness poll room.
        startTimeout: 60000,
        statusPollTimeout: 20000,
      },
    ],
  ],

  framework: "mocha",
  reporters: ["spec"],
  logLevel: "warn",
  bail: 0,
  waitforTimeout: 15000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  mochaOpts: { ui: "bdd", timeout: 90000 },

  // Point the WebView at the live dev server once per session, then wait for the
  // React app to mount before any spec runs.
  before: async function () {
    await browser.url("/");
    await browser.$("#root").waitForExist({ timeout: 20000 });
  },
};
