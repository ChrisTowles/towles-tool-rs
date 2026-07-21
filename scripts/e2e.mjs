#!/usr/bin/env node
// Orchestrates the WebdriverIO E2E run against the *real* Tauri shell.
//
// Ports come from the rendered `.env`/`.env.local` (same mechanism as
// `npm run dev`): TT_DEV_PORT is the Vite dev-server port; the embedded
// WebDriver port is the .env claim TT_E2E_WEBDRIVER_PORT, else
// TT_DEV_PORT + 3000. Nothing is hardcoded.
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
import { Result } from "better-result";
import { PortNeverListened, SpawnFailed } from "./errors.mjs";
import { requireDevPort, resolveWebdriverPort } from "./task-port.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const noBuild = process.argv.includes("--no-build");

const devPort = requireDevPort(repoRoot, { tag: "e2e" });
const wdPort = resolveWebdriverPort(devPort);

/**
 * @param {number} port
 * @param {string} host
 * @returns {Promise<boolean>}
 */
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
/**
 * @param {number} port
 * @param {number} timeoutMs
 * @returns {Promise<Result<void, PortNeverListened>>}
 */
function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  /** @type {Promise<Result<void, PortNeverListened>>} */
  return new Promise((resolve) => {
    const attempt = async () => {
      const [v4, v6] = await Promise.all([
        tryConnect(port, "127.0.0.1"),
        tryConnect(port, "::1"),
      ]);
      if (v4 || v6) return resolve(Result.ok(undefined));
      if (Date.now() > deadline) {
        return resolve(Result.err(new PortNeverListened({ port, timeoutMs })));
      }
      setTimeout(attempt, 250);
    };
    attempt();
  });
}

/**
 * Run a command to completion, inheriting stdio. A non-zero exit is an
 * ordinary result (the `number`); only failing to *launch* is an error —
 * without that split, a missing `cargo`/`npx` exits 1 with nothing said.
 *
 * @param {string} cmd
 * @param {string[]} args
 * @param {import("node:child_process").SpawnSyncOptions} [opts]
 * @returns {Result<number, SpawnFailed>}
 */
function run(cmd, args, opts) {
  const res = spawnSync(cmd, args, {
    stdio: "inherit",
    cwd: repoRoot,
    ...opts,
  });
  if (res.error) return Result.err(new SpawnFailed({ command: cmd, cause: res.error }));
  return Result.ok(res.status ?? 1);
}

/** @type {import("node:child_process").ChildProcess | undefined} */
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
    const built = run("cargo", ["build", "-p", "tt-app", "--features", "wdio"], {
      env: {
        ...process.env,
        TAURI_CONFIG: JSON.stringify({
          build: { devUrl: `http://localhost:${devPort}` },
        }),
      },
    });
    if (built.isErr()) die(built.error.message);
    if (built.value !== 0) process.exit(built.value);
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

  const up = await waitForPort(devPort, 30000);
  if (up.isErr()) die(up.error.message);

  console.log("[e2e] running wdio …");
  const ran = run("npx", ["wdio", "run", "e2e/wdio.conf.ts"], {
    env: {
      ...process.env,
      // Forced scope = full state isolation (shared stores included) — the
      // spawned app can never read or write the real settings/repos files.
      TT_STATE_SCOPE: `e2e-${path.basename(repoRoot)}`,
      TT_DEV_PORT: String(devPort),
      TT_E2E_WEBDRIVER_PORT: String(wdPort),
      // The service's execute/mock channel (DirectEvalClient) reads the eval
      // server port from TAURI_WEBDRIVER_PORT (default 4445); align it with the
      // embedded server port or browser.tauri.execute() hits the wrong port.
      TAURI_WEBDRIVER_PORT: String(wdPort),
      TAURI_WEBVIEW_AUTOMATION: "true",
      // The e2e suite is a hands-off verification run — don't let the
      // spawned window steal OS focus.
      TT_NO_FOCUS_STEAL: "1",
    },
  });
  if (ran.isErr()) die(ran.error.message);
  cleanup();
  process.exit(ran.value);
}

/**
 * Report, tear the Vite server down, and exit non-zero — the one terminal
 * boundary for every failure this script can hit.
 *
 * @param {string} message
 * @returns {never}
 */
function die(message) {
  console.error("[e2e]", message);
  cleanup();
  process.exit(1);
}

await main();
