// Per-task Vite dev-server port, from the task's rendered `.env`.
//
// Multiple checkouts of this repo (the main checkout, .claude/worktrees/thing,
// …) run `tauri dev` at the same time, so every checkout needs its own port.
// The single source of truth is the `${tt:port}` claim `tt task env` renders
// into `.env` (TT_DEV_PORT) — claims are unique across sibling checkouts by
// construction. There is deliberately NO derived/hashed fallback here: a port
// picked outside the claim system can collide with a sibling's claim, and
// `killPort` would then kill that sibling's legitimate dev server. A checkout
// with no rendered `.env` must run `tt task env` (the launchers do this
// automatically — see `requireDevPort`) or pin TT_DEV_PORT in `.env.local`.
import { basename, dirname, join } from "node:path";
import { readFileSync } from "node:fs";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { Result } from "better-result";
import { DevPortInvalid, DevPortUnset, EnvFileUnreadable, TaskEnvRenderFailed } from "./errors.mjs";

/**
 * Read an env file's contents, distinguishing "not there" (`null` — the normal
 * case for a checkout with no `.env.local`) from "there but unreadable", which
 * is a real misconfiguration a caller should not silently skip past.
 *
 * @param {string} path
 * @returns {Result<string | null, EnvFileUnreadable>}
 */
function readEnvFile(path) {
  try {
    return Result.ok(readFileSync(path, "utf8"));
  } catch (e) {
    if (/** @type {NodeJS.ErrnoException} */ (e)?.code === "ENOENT") return Result.ok(null);
    return Result.err(new EnvFileUnreadable({ path, cause: e }));
  }
}

/**
 * Load `.env.local` then `.env` (at `repoRoot`) into `process.env` so per-task
 * values like `TT_DEV_PORT` reach dev tooling without shell exports.
 * Precedence: real env vars > `.env.local` (manual pin) > `.env` (rendered by
 * `tt task new`/`env` with the task's port claims) — standard dotenv
 * layering, so a hand pin always beats the tool-rendered claim. Missing files
 * are a no-op; an unreadable one is an {@link EnvFileUnreadable}.
 *
 * @param {string} repoRoot
 * @returns {Result<void, EnvFileUnreadable>}
 */
export function loadEnvFiles(repoRoot) {
  for (const file of [".env.local", ".env"]) {
    const read = readEnvFile(join(repoRoot, file));
    if (read.isErr()) return Result.err(read.error);
    const raw = read.value;
    if (raw === null) continue;
    for (const line of raw.split("\n")) {
      const match = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$/);
      if (!match) continue;
      const key = match[1];
      let value = match[2];
      if (key === undefined || value === undefined) continue;
      if (
        (value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'"))
      ) {
        value = value.slice(1, -1);
      }
      if (!(key in process.env)) process.env[key] = value;
    }
  }
  return Result.ok(undefined);
}

/**
 * Resolve the dev-server port after loading the env files: `TT_DEV_PORT`
 * from shell env, `.env.local`, or the rendered `.env` (that precedence).
 *
 * The two ways to have no port are separate errors because callers act on
 * them differently: {@link DevPortUnset} is what a fresh checkout looks like
 * and `requireDevPort` recovers from it by rendering the task's `.env`, while
 * {@link DevPortInvalid} is a typo only the user can fix.
 *
 * @param {string} repoRoot
 * @returns {Result<number, DevPortUnset | DevPortInvalid | EnvFileUnreadable>}
 */
export function resolveDevPort(repoRoot) {
  const loaded = loadEnvFiles(repoRoot);
  if (loaded.isErr()) return Result.err(loaded.error);
  const override = process.env.TT_DEV_PORT;
  if (override === undefined || override === "") return Result.err(new DevPortUnset());
  const port = Number(override);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    return Result.err(new DevPortInvalid({ value: override }));
  }
  return Result.ok(port);
}

/**
 * The `tt task env` name for this checkout: the task's dir name when it sits
 * at `<main>/.claude/worktrees/<name>`, else `primary` (the main checkout).
 *
 * @param {string} repoRoot
 * @returns {string}
 */
export function taskEnvName(repoRoot) {
  const worktrees = dirname(repoRoot);
  const claude = dirname(worktrees);
  return basename(worktrees) === "worktrees" && basename(claude) === ".claude"
    ? basename(repoRoot)
    : "primary";
}

/**
 * Run `tt task env <name>` in `repoRoot` to claim this checkout's ports and
 * render its `.env`.
 *
 * @param {string} repoRoot
 * @param {string} name
 * @returns {Result<void, TaskEnvRenderFailed>}
 */
function renderTaskEnv(repoRoot, name) {
  return Result.try({
    try: () => {
      execFileSync("tt", ["task", "env", name], { cwd: repoRoot, stdio: "inherit" });
    },
    catch: (e) => new TaskEnvRenderFailed({ name, cause: e }),
  });
}

/**
 * Resolve the dev port or die trying. Invalid `TT_DEV_PORT` → error + exit.
 * Unset with `render: true` (launchers: dev/dev:drive) → run
 * `tt task env <this checkout>` to claim ports, then re-resolve; unset
 * otherwise (or when the render fails) → exit with instructions. The
 * returned port is always an explicit claim or pin, so `killPort` on it is
 * safe — it can only be this checkout's own orphaned session.
 *
 * @param {string} repoRoot
 * @param {{ tag?: string; render?: boolean }} [opts]
 * @returns {number}
 */
export function requireDevPort(repoRoot, { tag = "task-port", render = false } = {}) {
  const name = taskEnvName(repoRoot);

  /** @param {ReturnType<typeof resolveDevPort>} resolved */
  const die = (resolved) => {
    if (resolved.isErr() && !DevPortUnset.is(resolved.error)) {
      console.error(`[${tag}] ${resolved.error.message}`);
      process.exit(1);
    }
    console.error(
      `[${tag}] no TT_DEV_PORT for this checkout — run \`tt task env ${name}\` to claim ports, ` +
        `or pin TT_DEV_PORT in .env.local`,
    );
    process.exit(1);
  };

  let resolved = resolveDevPort(repoRoot);
  if (resolved.isOk()) return resolved.value;
  if (!DevPortUnset.is(resolved.error)) return die(resolved);

  if (!render) return die(resolved);

  console.log(`[${tag}] no TT_DEV_PORT yet — rendering .env via \`tt task env ${name}\``);
  const rendered = renderTaskEnv(repoRoot, name);
  // `tt` missing or the render failed — say so, then fall through to the
  // instructions below rather than exiting on a recoverable step.
  if (rendered.isErr()) console.error(`[${tag}] ${rendered.error.message}`);

  resolved = resolveDevPort(repoRoot);
  return resolved.isOk() ? resolved.value : die(resolved);
}

/**
 * The embedded WebDriver server's port for a given dev port: an explicit
 * `TT_E2E_WEBDRIVER_PORT` (shell env or an env file) wins, otherwise
 * `devPort + 3000`. Shared so `dev:drive`/`drive`/`e2e` and `wdio.conf.ts`
 * agree on one convention instead of each hardcoding the offset.
 *
 * @param {number} devPort
 * @returns {number}
 */
export function resolveWebdriverPort(devPort) {
  return Number(process.env.TT_E2E_WEBDRIVER_PORT) || devPort + 3000;
}

// A port is free only if BOTH loopback stacks are bindable: another task may
// hold it on IPv6 (::1) while IPv4 (127.0.0.1) looks open. Only EADDRINUSE
// counts as "taken" — plus EACCES, a privileged port the dev server couldn't
// bind either; other errors (e.g. no IPv6) don't.
// Mirrors `port_occupied` in crates/tt-tasks/src/ops.rs — keep the two in sync.
/**
 * @param {number} port
 * @returns {Promise<boolean>}
 */
export function isPortFree(port) {
  /** @param {string} host @returns {Promise<boolean>} */
  const tryHost = (host) =>
    new Promise((resolve) => {
      const server = createServer();
      server.once("error", (err) =>
        resolve(
          !["EADDRINUSE", "EACCES"].includes(
            /** @type {NodeJS.ErrnoException} */ (err).code ?? "",
          ),
        ),
      );
      server.once("listening", () => server.close(() => resolve(true)));
      server.listen(port, host);
    });
  return Promise.all([tryHost("127.0.0.1"), tryHost("::1")]).then((results) =>
    results.every(Boolean),
  );
}

/**
 * @param {number} port
 * @returns {string[]}
 */
function listeningPids(port) {
  try {
    const out = execFileSync("lsof", ["-ti", `tcp:${port}`, "-sTCP:LISTEN"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
    return [...new Set(out.split("\n").map((s) => s.trim()).filter(Boolean))];
  } catch {
    // Not an error worth reporting: lsof exits 1 when nothing matches, which
    // is the expected answer on a free port, and an lsof-less platform has no
    // pids to offer either. Both mean "nothing to kill".
    return [];
  }
}

/**
 * A process's group id, or `null` when there isn't a signalable one.
 *
 * Rejects a pgid below 2 rather than returning it: `killPort` negates this
 * value for `process.kill`, where the two smallest inputs aren't process
 * groups but wildcards — `kill(0)` signals *this* script's own group, and
 * `kill(-1)` signals *every process the user can signal*. `ps -o pgid=`
 * really does report 0 (kernel threads do), so this is a guard, not a
 * formality. Mirrors the same check in `tt_tasks::ports::parse_ps_row`.
 *
 * @param {string} pid
 * @returns {string | null}
 */
function pgidOf(pid) {
  try {
    const pgid = execFileSync("ps", ["-o", "pgid=", "-p", pid], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    return Number(pgid) >= 2 ? pgid : null;
  } catch {
    return null; // process already gone
  }
}

/**
 * @param {number} port
 * @param {number} timeoutMs
 * @param {number} pollMs
 * @returns {Promise<boolean>}
 */
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
 * a port this task deterministically owns (an explicit `TT_DEV_PORT`, or
 * `dev:drive`'s always-pinned port) — never call this on a scanned/shared
 * port, since whatever's listening there may be another task's legitimate
 * dev server. No-op on Windows (no lsof/ps/POSIX process groups) and when
 * nothing is listening.
 *
 * @param {number} port
 * @returns {Promise<void>}
 */
export async function killPort(port) {
  if (process.platform === "win32") return;
  const pids = listeningPids(port);
  if (!pids.length) return;

  /** @type {Set<string>} */
  const pgids = new Set(pids.map(pgidOf).filter((pgid) => pgid !== null));
  if (!pgids.size) return;

  console.log(
    `[task-port] port ${port} is already in use — stopping it (pgid ${[...pgids].join(", ")})`,
  );
  for (const pgid of pgids) {
    try {
      process.kill(-Number(pgid), "SIGTERM");
    } catch {
      // already gone
    }
  }

  if (await waitUntilFree(port, 3000, 100)) return;

  console.log(`[task-port] port ${port} still in use after SIGTERM — sending SIGKILL`);
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
 * `npm run dev`/`dev:drive` in this task then fails with "port already in
 * use" until someone finds and kills the leftovers by hand. Forwarding
 * SIGINT/SIGTERM/SIGHUP from this wrapper to that whole group means the
 * normal case (Ctrl+C in the terminal this was started from) tears
 * everything down together, and a still-alive wrapper can always be killed
 * by itself to the same effect — but the group id is the one thing that's
 * always reliable, even if the wrapper itself is already gone (see
 * e2e/README.md's "stopping a stray dev session").
 *
 * @param {string[]} args
 * @param {NodeJS.ProcessEnv} env
 * @returns {import("node:child_process").ChildProcess}
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
    /** @param {NodeJS.Signals} signal */
    const forward = (signal) => {
      if (!child.pid) return;
      try {
        process.kill(-child.pid, signal);
      } catch {
        // Already gone.
      }
    };
    /** @type {NodeJS.Signals[]} */
    const signals = ["SIGINT", "SIGTERM", "SIGHUP"];
    for (const signal of signals) {
      process.on(signal, () => forward(signal));
    }
  }

  child.on("exit", (code) => process.exit(code ?? 0));
  return child;
}
