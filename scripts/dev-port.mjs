#!/usr/bin/env node
// Picks the port for the Vite dev server before launching `tauri dev`, so
// running this repo from multiple worktree slots at once doesn't collide.
//
// Port resolution, in order:
//   1. TT_DEV_PORT — an explicit override (shell env or `.env.local` at the
//      repo root). Used as-is.
//   2. Otherwise scan upward from this slot's deterministic base port (derived
//      from the repo-root directory name) for a free port, so different slots
//      start in different ranges instead of all racing for 1420.
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { slotBasePort, loadEnvLocal } from "./slot-port.mjs";

const MAX_ATTEMPTS = 100;
const PORT_ENV = "TT_DEV_PORT";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

// A port is free only if BOTH loopback stacks are bindable: another slot may
// hold it on IPv6 (::1) while IPv4 (127.0.0.1) looks open, and vite would still
// collide. Only EADDRINUSE counts as "taken"; other errors (e.g. no IPv6) don't.
function isPortFree(port) {
  const tryHost = (host) =>
    new Promise((resolve) => {
      const server = createServer();
      server.once("error", (err) => resolve(err.code !== "EADDRINUSE"));
      server.once("listening", () => server.close(() => resolve(true)));
      server.listen(port, host);
    });
  return Promise.all([tryHost("127.0.0.1"), tryHost("::1")]).then((results) =>
    results.every(Boolean),
  );
}

async function findFreePort(start) {
  for (let port = start; port < start + MAX_ATTEMPTS; port++) {
    if (await isPortFree(port)) return port;
  }
  throw new Error(
    `no free port found in range ${start}-${start + MAX_ATTEMPTS}`,
  );
}

loadEnvLocal(repoRoot);

let port;
const override = process.env[PORT_ENV];
if (override !== undefined && override !== "") {
  port = Number(override);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    console.error(
      `[dev-port] ${PORT_ENV}=${override} is not a valid port (1-65535)`,
    );
    process.exit(1);
  }
  console.log(`[dev-port] using ${PORT_ENV}=${port}`);
} else {
  const base = slotBasePort(repoRoot);
  port = await findFreePort(base);
  console.log(
    `[dev-port] using port ${port} (slot base ${base}; set ${PORT_ENV} in .env.local to pin one)`,
  );
}

const child = spawn(
  "tauri",
  [
    "dev",
    "--config",
    JSON.stringify({ build: { devUrl: `http://localhost:${port}` } }),
  ],
  {
    stdio: "inherit",
    env: { ...process.env, TT_DEV_PORT: String(port) },
    shell: process.platform === "win32",
  },
);

child.on("exit", (code) => process.exit(code ?? 0));
