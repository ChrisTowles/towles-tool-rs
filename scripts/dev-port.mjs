#!/usr/bin/env node
// Picks a free port for the Vite dev server before launching `tauri dev`,
// so running this repo from multiple worktree slots at once doesn't collide
// on the hardcoded 1420 (see docs/UI-SCREENSHOTS.md).
import { spawn } from "node:child_process";
import { createServer } from "node:net";

const BASE_PORT = 1420;
const MAX_ATTEMPTS = 100;

function isPortFree(port) {
  return new Promise((resolve) => {
    const server = createServer();
    server.once("error", () => resolve(false));
    server.once("listening", () => server.close(() => resolve(true)));
    server.listen(port, "127.0.0.1");
  });
}

async function findFreePort(start) {
  for (let port = start; port < start + MAX_ATTEMPTS; port++) {
    if (await isPortFree(port)) return port;
  }
  throw new Error(`no free port found in range ${start}-${start + MAX_ATTEMPTS}`);
}

const port = await findFreePort(BASE_PORT);
console.log(`[dev-port] using port ${port}`);

const child = spawn(
  "tauri",
  ["dev", "--config", JSON.stringify({ build: { devUrl: `http://localhost:${port}` } })],
  {
    stdio: "inherit",
    env: { ...process.env, TT_DEV_PORT: String(port) },
    shell: process.platform === "win32",
  },
);

child.on("exit", (code) => process.exit(code ?? 0));
