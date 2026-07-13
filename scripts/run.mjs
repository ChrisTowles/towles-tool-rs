#!/usr/bin/env node
// Builds tt-app in release mode (optimized, no `cargo build --debug`
// slowness) and runs the resulting binary directly — no installer bundling.
// Debug builds (`npm run dev`) are fine for iterating, but their unoptimized
// terminal rendering + IPC path is visibly laggy under everyday use (scroll,
// typing) once several worktree slots + agent sessions are running at once.
// This is the "just run it fast" counterpart to `npm run dev`.
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const build = spawnSync(
  "tauri",
  ["build", "--no-bundle"],
  { stdio: "inherit", cwd: repoRoot, shell: process.platform === "win32" },
);
if (build.status !== 0) process.exit(build.status ?? 1);

const binName = process.platform === "win32" ? "tt-app.exe" : "tt-app";
const bin = path.join(repoRoot, "target", "release", binName);

const child = spawn(bin, { stdio: "inherit", cwd: repoRoot });
child.on("exit", (code) => process.exit(code ?? 0));
