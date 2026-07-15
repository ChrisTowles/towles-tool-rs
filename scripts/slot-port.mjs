// Deterministic per-slot Vite dev-server port.
//
// Multiple checkouts of this repo (towles-tool-rs-primary, slots/thing, …) run
// `tauri dev` at the same time. If every slot defaults to 1420 they collide, so
// we derive a stable base port from the slot's repo-root directory name: each
// slot prefers its own port and keeps it run-to-run, and `dev-port.mjs` scans
// upward from there for a free one as a safety net. An explicit TT_DEV_PORT
// (shell env, `.env.local` pin, or `.env` rendered by `tt slot`) wins over this.
import { basename, join } from "node:path";
import { readFileSync } from "node:fs";
import { spawn } from "node:child_process";

export const PORT_MIN = 1420;
const PORT_SPAN = 200; // ports 1420–1619, partitioned across slots by name hash

/** Stable base port for the slot rooted at `repoRoot` (keyed on its dir name). */
export function slotBasePort(repoRoot) {
  const name = basename(repoRoot);
  let hash = 0;
  for (let i = 0; i < name.length; i++)
    hash = (hash * 31 + name.charCodeAt(i)) | 0;
  return PORT_MIN + (Math.abs(hash) % PORT_SPAN);
}

/**
 * Load `.env.local` then `.env` (at `repoRoot`) into `process.env` so per-slot
 * values like `TT_DEV_PORT` reach dev tooling without shell exports.
 * Precedence: real env vars > `.env.local` (manual pin) > `.env` (rendered by
 * `tt slot new`/`env` with the slot's port claims) — standard dotenv
 * layering, so a hand pin always beats the tool-rendered claim. Missing files
 * are a no-op.
 */
export function loadEnvFiles(repoRoot) {
  for (const file of [".env.local", ".env"]) {
    let raw;
    try {
      raw = readFileSync(join(repoRoot, file), "utf8");
    } catch {
      continue;
    }
    for (const line of raw.split("\n")) {
      const match = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$/);
      if (!match) continue;
      const key = match[1];
      let value = match[2];
      if (
        (value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))
      ) {
        value = value.slice(1, -1);
      }
      if (!(key in process.env)) process.env[key] = value;
    }
  }
}

/**
 * Resolve the dev-server port after loading the env files: an explicit
 * `TT_DEV_PORT` (shell env, `.env.local`, or rendered `.env`) wins, otherwise
 * the slot base port. Returns `null` for an invalid `TT_DEV_PORT` so the
 * caller can decide.
 */
export function resolveDevPort(repoRoot) {
  loadEnvFiles(repoRoot);
  const override = process.env.TT_DEV_PORT;
  if (override !== undefined && override !== "") {
    const port = Number(override);
    if (!Number.isInteger(port) || port <= 0 || port > 65535) return null;
    return port;
  }
  return slotBasePort(repoRoot);
}

/**
 * The embedded WebDriver server's port for a given dev port: an explicit
 * `TT_E2E_WEBDRIVER_PORT` (shell env or an env file) wins, otherwise
 * `devPort + 3000`. Shared so `dev:drive`/`drive`/`e2e` and `wdio.conf.ts`
 * agree on one convention instead of each hardcoding the offset.
 */
export function resolveWebdriverPort(devPort) {
  return Number(process.env.TT_E2E_WEBDRIVER_PORT) || devPort + 3000;
}

/**
 * Spawn `tauri dev` (or `dev` with extra flags) as the leader of its own
 * process group, so its whole subtree — the `tauri` CLI's own node shim,
 * `cargo run`, `tt-app`, and the `vite`/esbuild dev server `beforeDevCommand`
 * launches — always shares one group id (== this process's pid), no matter
 * how many shell layers npm/tauri interpose. Without this, a signal aimed at
 * only the one pid you can see (e.g. killing what `ps` shows for "tauri dev")
 * leaves vite/esbuild orphaned and still bound to the dev port — the next
 * `npm run dev`/`dev:drive` in this slot then fails with "port already in
 * use" until someone finds and kills the leftovers by hand. Forwarding
 * SIGINT/SIGTERM/SIGHUP from this wrapper to that whole group means the
 * normal case (Ctrl+C in the terminal this was started from) tears
 * everything down together, and a still-alive wrapper can always be killed
 * by itself to the same effect — but the group id is the one thing that's
 * always reliable, even if the wrapper itself is already gone (see
 * e2e/README.md's "stopping a stray dev session").
 */
export function spawnTauriDev(args, env) {
  const posix = process.platform !== "win32";
  const child = spawn("tauri", args, {
    stdio: "inherit",
    env,
    shell: !posix,
    // Windows has no POSIX process groups; `detached` there just means "own
    // console", which isn't what we want, so leave it plain and rely on the
    // OS's own console Ctrl+C propagation (existing behavior, unchanged).
    detached: posix,
  });

  if (posix) {
    const forward = (signal) => {
      if (!child.pid) return;
      try {
        process.kill(-child.pid, signal);
      } catch {
        // Already gone.
      }
    };
    for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
      process.on(signal, () => forward(signal));
    }
  }

  child.on("exit", (code) => process.exit(code ?? 0));
  return child;
}
