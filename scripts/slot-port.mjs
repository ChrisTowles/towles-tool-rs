// Deterministic per-slot Vite dev-server port.
//
// Multiple checkouts of this repo (the main checkout, .claude/worktrees/thing,
// …) run `tauri dev` at the same time. If every slot defaults to 1420 they
// collide, so we derive a stable base port from the checkout's directory name
// (the worktree dir's basename — unchanged by the nesting): each slot gets
// its own port and keeps it run-to-run — no scanning for a free one, since
// that would just paper over a stuck orphaned process instead of clearing
// it (see `killPort`). An explicit TT_DEV_PORT (shell env, `.env.local` pin,
// or `.env` rendered by `tt slot`) wins over the base port.
import { basename, join } from "node:path";
import { readFileSync } from "node:fs";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";

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

// A port is free only if BOTH loopback stacks are bindable: another slot may
// hold it on IPv6 (::1) while IPv4 (127.0.0.1) looks open. Only EADDRINUSE
// counts as "taken"; other errors (e.g. no IPv6) don't.
export function isPortFree(port) {
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

function listeningPids(port) {
  try {
    const out = execFileSync("lsof", ["-ti", `tcp:${port}`, "-sTCP:LISTEN"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    return [...new Set(out.split("\n").map((s) => s.trim()).filter(Boolean))];
  } catch {
    return []; // lsof missing, or exits 1 when nothing matches
  }
}

function pgidOf(pid) {
  try {
    return execFileSync("ps", ["-o", "pgid=", "-p", pid], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null; // process already gone
  }
}

async function waitUntilFree(port, timeoutMs, pollMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await isPortFree(port)) return true;
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }
  return isPortFree(port);
}

/**
 * Kill whatever's already listening on `port` — whole process group, not
 * just the one pid `lsof` reports — before we bind there ourselves.
 * `spawnTauriDev` groups tauri/cargo/tt-app/vite under one pgid so a normal
 * Ctrl+C tears it all down together, but a session killed by pid alone (e.g.
 * a backgrounded terminal closed without Ctrl+C) leaves vite/esbuild
 * orphaned and still bound to the port — see e2e/README.md's "stopping a
 * stray dev session" for the manual version of this recovery. Only meant for
 * a port this slot deterministically owns (an explicit `TT_DEV_PORT`, or
 * `dev:drive`'s always-pinned port) — never call this on a scanned/shared
 * port, since whatever's listening there may be another slot's legitimate
 * dev server. No-op on Windows (no lsof/ps/POSIX process groups) and when
 * nothing is listening.
 */
export async function killPort(port) {
  if (process.platform === "win32") return;
  const pids = listeningPids(port);
  if (!pids.length) return;

  const pgids = new Set(pids.map(pgidOf).filter(Boolean));
  if (!pgids.size) return;

  console.log(
    `[slot-port] port ${port} is already in use — stopping it (pgid ${[...pgids].join(", ")})`,
  );
  for (const pgid of pgids) {
    try {
      process.kill(-Number(pgid), "SIGTERM");
    } catch {
      // already gone
    }
  }

  if (await waitUntilFree(port, 3000, 100)) return;

  console.log(`[slot-port] port ${port} still in use after SIGTERM — sending SIGKILL`);
  for (const pgid of pgids) {
    try {
      process.kill(-Number(pgid), "SIGKILL");
    } catch {
      // already gone
    }
  }
  await waitUntilFree(port, 2000, 100);
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
