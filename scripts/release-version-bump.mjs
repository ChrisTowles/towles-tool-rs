#!/usr/bin/env node
// Bumps the app's release version across every file that carries it, then
// syncs the two lockfiles so `npm ci`/`cargo build` see a consistent tree.
// The one version that matters here is the *app's* — `tauri.conf.json` and
// `crates-tauri/tt-app/Cargo.toml` (see release.yml's header comment: bump
// before tagging, the workflow doesn't do it for you) — plus the two
// `package.json`s, which track the same number for convenience. It
// deliberately does NOT touch the independent `0.1.0` versions on the
// library crates under `crates/` — those are internal, unpublished, and
// unrelated to what gets tagged and released.
//
// Usage:
//   node scripts/release-version-bump.mjs <major|minor|patch|x.y.z>
//
// Rewrites files + regenerates both lockfiles, then leaves everything
// unstaged for review — this script never runs git add/commit/tag/push.
// Follow up by hand once the diff looks right:
//   git add -A && git commit -m "chore(release): bump version to vX.Y.Z"
//   git tag vX.Y.Z && git push origin vX.Y.Z
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { Result } from "better-result";
import { BadVersion, VersionLineMissing } from "./errors.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/**
 * A file that carries the app's release version, and how its version line is
 * shaped.
 * @typedef {{ path: string; format: "json" | "toml" }} VersionFile
 */

/** @type {VersionFile[]} */
export const VERSION_FILES = [
  { path: "package.json", format: "json" },
  { path: "apps/client/package.json", format: "json" },
  { path: "crates-tauri/tt-app/tauri.conf.json", format: "json" },
  { path: "crates-tauri/tt-app/Cargo.toml", format: "toml" },
];

/**
 * Parses a `major.minor.patch` string into its numeric parts.
 *
 * @param {string} version
 * @returns {Result<[number, number, number], BadVersion>}
 */
export function parseVersion(version) {
  const parts = version.split(".").map(Number);
  if (parts.length !== 3 || parts.some((n) => !Number.isInteger(n) || n < 0)) {
    return Result.err(new BadVersion({ version }));
  }
  return Result.ok(/** @type {[number, number, number]} */ (parts));
}

/**
 * Resolves the CLI argument (`major`/`minor`/`patch`, or an explicit
 * `x.y.z`) against the current version.
 *
 * @param {string} current
 * @param {string} arg
 * @returns {Result<string, BadVersion>}
 */
export function resolveNewVersion(current, arg) {
  const parsed = parseVersion(current);
  if (parsed.isErr()) return parsed;
  const [major, minor, patch] = parsed.value;
  switch (arg) {
    case "major":
      return Result.ok(`${major + 1}.0.0`);
    case "minor":
      return Result.ok(`${major}.${minor + 1}.0`);
    case "patch":
      return Result.ok(`${major}.${minor}.${patch + 1}`);
    default:
      return parseVersion(arg).map(() => arg);
  }
}

/**
 * The exact-text needle for a file's version line, by format.
 *
 * @param {VersionFile["format"]} format
 * @param {string} version
 * @returns {string}
 */
function needle(format, version) {
  return format === "toml" ? `version = "${version}"` : `"version": "${version}"`;
}

/**
 * Rewrites a single file's version line, preserving all other formatting.
 * Only the first occurrence is replaced — every file in {@link VERSION_FILES}
 * has exactly one top-level `version` field.
 *
 * @param {string} contents
 * @param {VersionFile["format"]} format
 * @param {string} from
 * @param {string} to
 * @returns {Result<string, VersionLineMissing>}
 */
export function withBumpedVersion(contents, format, from, to) {
  const from_ = needle(format, from);
  const index = contents.indexOf(from_);
  if (index === -1) return Result.err(new VersionLineMissing({ needle: from_ }));
  return Result.ok(
    contents.slice(0, index) + needle(format, to) + contents.slice(index + from_.length),
  );
}

/**
 * @param {string[]} args
 * @param {string} cwd
 */
function run(args, cwd) {
  console.log(`[release-version-bump] $ ${args.join(" ")}`);
  execFileSync(args[0], args.slice(1), { cwd, stdio: "inherit" });
}

async function main() {
  const arg = process.argv[2];
  if (!arg) {
    console.error("usage: node scripts/release-version-bump.mjs <major|minor|patch|x.y.z>");
    process.exit(1);
  }

  const rootPkg = JSON.parse(readFileSync(path.join(repoRoot, "package.json"), "utf8"));
  const from = rootPkg.version;
  const resolved = resolveNewVersion(from, arg);
  if (resolved.isErr()) {
    console.error(`[release-version-bump] ${resolved.error.message}`);
    process.exit(1);
  }
  const to = resolved.value;

  for (const file of VERSION_FILES) {
    const abs = path.join(repoRoot, file.path);
    const contents = readFileSync(abs, "utf8");
    const rewritten = withBumpedVersion(contents, file.format, from, to);
    if (rewritten.isErr()) {
      console.error(`[release-version-bump] ${file.path}: ${rewritten.error.message}`);
      process.exit(1);
    }
    writeFileSync(abs, rewritten.value);
    console.log(`[release-version-bump] ${file.path}: ${from} -> ${to}`);
  }

  // Sync both lockfiles so the bump commit is self-consistent. `cargo check`
  // is enough to update Cargo.lock's `tt-app` entry without a full build;
  // `npm install --package-lock-only` does the equivalent for package-lock.json
  // without touching node_modules.
  run(["cargo", "check", "-p", "tt-app", "--quiet"], repoRoot);
  run(["npm", "install", "--package-lock-only", "--silent"], repoRoot);

  console.log(`\n[release-version-bump] done: ${from} -> ${to}. Review the diff, then:`);
  console.log(`  git add -A && git commit -m "chore(release): bump version to ${to}"`);
  console.log(`  git tag v${to} && git push origin v${to}`);
}

// Only run when invoked directly (`node scripts/release-version-bump.mjs`),
// not when imported by the test file.
if (path.resolve(fileURLToPath(import.meta.url)) === path.resolve(process.argv[1] ?? "")) {
  main();
}
