// Deterministic per-slot Vite dev-server port.
//
// Multiple worktree slots of this repo (towles-tool-rs-slot-0, -slot-1, …) run
// `tauri dev` at the same time. If every slot defaults to 1420 they collide, so
// we derive a stable base port from the slot's repo-root directory name: each
// slot prefers its own port and keeps it run-to-run, and `dev-port.mjs` scans
// upward from there for a free one as a safety net. An explicit TT_DEV_PORT /
// `.env.local` pin still wins over this.
import { basename, join } from "node:path";
import { readFileSync } from "node:fs";

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
 * Load `.env.local` (at `repoRoot`) into `process.env` so per-slot values like
 * `TT_DEV_PORT` can be pinned without exporting them in the shell. Real env vars
 * win over the file; a missing file is a no-op.
 */
export function loadEnvLocal(repoRoot) {
  let raw;
  try {
    raw = readFileSync(join(repoRoot, ".env.local"), "utf8");
  } catch {
    return;
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

/**
 * Resolve the dev-server port after loading `.env.local`: an explicit
 * `TT_DEV_PORT` (shell env or `.env.local`) wins, otherwise the slot base port.
 * Returns `null` for an invalid `TT_DEV_PORT` so the caller can decide.
 */
export function resolveDevPort(repoRoot) {
  loadEnvLocal(repoRoot);
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
 * `TT_E2E_WEBDRIVER_PORT` (shell env or `.env.local`) wins, otherwise
 * `devPort + 3000`. Shared so `dev:drive`/`drive`/`e2e` and `wdio.conf.ts`
 * agree on one convention instead of each hardcoding the offset.
 */
export function resolveWebdriverPort(devPort) {
  return Number(process.env.TT_E2E_WEBDRIVER_PORT) || devPort + 3000;
}
