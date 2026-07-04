#!/usr/bin/env node
// Picks the port for the Vite dev server before launching `tauri dev`, so
// running this repo from multiple worktree slots at once doesn't collide on the
// hardcoded 1420 (see docs/UI-SCREENSHOTS.md).
//
// Port resolution, in order:
//   1. TT_DEV_PORT — an explicit override (shell env or `.env.local` at the
//      repo root). Pins a deterministic per-slot port; used as-is.
//   2. Otherwise scan upward from 1420 for a free port.
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const BASE_PORT = 1420;
const MAX_ATTEMPTS = 100;
const PORT_ENV = "TT_DEV_PORT";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// Load `.env.local` (repo root) into process.env so a per-slot port can be
// pinned without exporting it in the shell. Real env vars win over the file.
function loadEnvLocal() {
  let raw;
  try {
    raw = readFileSync(path.join(repoRoot, ".env.local"), "utf8");
  } catch {
    return; // no .env.local — fine.
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
  throw new Error(`no free port found in range ${start}-${start + MAX_ATTEMPTS}`);
}

loadEnvLocal();

let port;
const override = process.env[PORT_ENV];
if (override !== undefined && override !== "") {
  port = Number(override);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    console.error(`[dev-port] ${PORT_ENV}=${override} is not a valid port (1-65535)`);
    process.exit(1);
  }
  console.log(`[dev-port] using ${PORT_ENV}=${port}`);
} else {
  port = await findFreePort(BASE_PORT);
  console.log(`[dev-port] using port ${port} (set ${PORT_ENV} in .env.local to pin one)`);
}

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
