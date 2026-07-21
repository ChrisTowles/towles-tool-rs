#!/usr/bin/env node
// Picks the port for the Vite dev server before launching `tauri dev`, so
// running this repo from multiple worktree tasks at once doesn't collide.
//
// The port is always an explicit per-checkout claim, never scanned or
// derived: TT_DEV_PORT from shell env, `.env.local` pin, or the `.env`
// rendered by `tt task env` (which `requireDevPort` runs for us when the
// checkout has no claim yet). Whatever's already listening there gets killed
// first (almost always this task's own orphaned dev session, since the port
// is claimed per-checkout, not shared) — see `killPort` in task-port.mjs. If
// it's still occupied after that, we fail rather than silently moving to a
// different port.
import { fileURLToPath } from "node:url";
import path from "node:path";
import { requireDevPort, spawnTauriDev, isPortFree, killPort } from "./task-port.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const port = requireDevPort(repoRoot, { tag: "dev-port", render: true });
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
