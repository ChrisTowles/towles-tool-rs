#!/usr/bin/env node
// Picks the port for the Vite dev server before launching `tauri dev`, so
// running this repo from multiple worktree slots at once doesn't collide.
//
// The port is always deterministic, never scanned: an explicit TT_DEV_PORT
// (shell env, `.env.local`, or rendered `.env` at the repo root) wins,
// otherwise this slot's stable base port (derived from the repo-root
// directory name — see `slotBasePort`). Whatever's already listening there
// gets killed first (almost always this slot's own orphaned dev session,
// since the port is pinned per-slot, not shared) — see `killPort` in
// slot-port.mjs. If it's still occupied after that, we fail rather than
// silently moving to a different port.
import { fileURLToPath } from "node:url";
import path from "node:path";
import { resolveDevPort, spawnTauriDev, isPortFree, killPort } from "./slot-port.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const port = resolveDevPort(repoRoot);
if (!port) {
  console.error(
    `[dev-port] TT_DEV_PORT=${process.env.TT_DEV_PORT} is not a valid port (1-65535)`,
  );
  process.exit(1);
}
console.log(`[dev-port] using port ${port} (set TT_DEV_PORT in .env.local to pin a different one)`);

await killPort(port);
if (!(await isPortFree(port))) {
  console.error(`[dev-port] port ${port} is still in use — couldn't free it, aborting`);
  process.exit(1);
}

spawnTauriDev(
  ["dev", "--config", JSON.stringify({ build: { devUrl: `http://localhost:${port}` } })],
  { ...process.env, TT_DEV_PORT: String(port) },
);
