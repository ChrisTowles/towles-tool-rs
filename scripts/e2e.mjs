#!/usr/bin/env node
// Orchestrates the WebdriverIO E2E run against the *real* Tauri shell.
//
// Ports come from `.env.local` (same mechanism as `npm run dev`): TT_DEV_PORT is
// the Vite dev-server port; the embedded WebDriver port defaults to
// TT_DEV_PORT + 3000 (override with TT_E2E_WEBDRIVER_PORT). Nothing is
// hardcoded.
//
// Steps:
//   1. resolve the dev port from .env.local,
//   2. build the app with the `wdio` cargo feature (skip with --no-build),
//   3. serve the wdio-enabled frontend on that port (VITE_WDIO=1),
//   4. run wdio with TAURI_WEBVIEW_AUTOMATION=true so the launched app's WebView
//      is automatable; wdio navigates it to the dev port (see wdio.conf.ts),
//   5. always tear the Vite server down.
import { spawn, spawnSync } from "node:child_process";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveDevPort, resolveWebdriverPort } from "./slot-port.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const noBuild = process.argv.includes("--no-build");

const devPort = resolveDevPort(repoRoot);
if (!devPort) {
  console.error(
    `[e2e] TT_DEV_PORT=${process.env.TT_DEV_PORT} is not a valid port`,
  );
  process.exit(1);
}
const wdPort = resolveWebdriverPort(devPort);

function tryConnect(port, host) {
  return new Promise((resolve) => {
    const socket = net.connect(port, host);
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => {
      socket.destroy();
      resolve(false);
    });
  });
}

// Vite may bind only one loopback stack (::1 or 127.0.0.1); treat the port as up
// if either accepts a connection.
function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const attempt = async () => {
      const [v4, v6] = await Promise.all([
        tryConnect(port, "127.0.0.1"),
        tryConnect(port, "::1"),
      ]);
      if (v4 || v6) return resolve();
      if (Date.now() > deadline)
        return reject(new Error(`port ${port} not up in ${timeoutMs}ms`));
      setTimeout(attempt, 250);
    };
    attempt();
  });
}

function run(cmd, args, opts) {
  const res = spawnSync(cmd, args, {
    stdio: "inherit",
    cwd: repoRoot,
    ...opts,
  });
  return res.status ?? 1;
}

let vite;
function cleanup() {
  if (vite && vite.pid) {
    try {
      process.kill(-vite.pid); // kill the detached process group
    } catch {
      /* already gone */
    }
    vite = undefined;
  }
}

async function main() {
  console.log(`[e2e] dev port ${devPort}, webdriver port ${wdPort}`);

  if (!noBuild) {
    console.log("[e2e] building app with --features wdio …");
    // Bake devUrl to the dev port so the WebView we navigate to is a trusted
    // Tauri origin — otherwise IPC invokes are rejected ("not allowed").
    const code = run("cargo", ["build", "-p", "tt-app", "--features", "wdio"], {
      env: {
        ...process.env,
        TAURI_CONFIG: JSON.stringify({
          build: { devUrl: `http://localhost:${devPort}` },
        }),
      },
    });
    if (code !== 0) process.exit(code);
  }

  console.log(`[e2e] starting Vite on ${devPort} …`);
  vite = spawn("npx", ["vite", "--port", String(devPort), "--strictPort"], {
    cwd: path.join(repoRoot, "apps/client"),
    env: { ...process.env, TT_DEV_PORT: String(devPort), VITE_WDIO: "1" },
    stdio: "inherit",
    detached: true,
  });
  process.on("exit", cleanup);
  process.on("SIGINT", () => {
    cleanup();
    process.exit(130);
  });
  process.on("SIGTERM", () => {
    cleanup();
    process.exit(143);
  });

  await waitForPort(devPort, 30000);

  console.log("[e2e] running wdio …");
  const code = run("npx", ["wdio", "run", "e2e/wdio.conf.ts"], {
    env: {
      ...process.env,
      TT_DEV_PORT: String(devPort),
      TT_E2E_WEBDRIVER_PORT: String(wdPort),
      // The service's execute/mock channel (DirectEvalClient) reads the eval
      // server port from TAURI_WEBDRIVER_PORT (default 4445); align it with the
      // embedded server port or browser.tauri.execute() hits the wrong port.
      TAURI_WEBDRIVER_PORT: String(wdPort),
      TAURI_WEBVIEW_AUTOMATION: "true",
    },
  });
  cleanup();
  process.exit(code);
}

main().catch((err) => {
  console.error("[e2e]", err.message);
  cleanup();
  process.exit(1);
});
