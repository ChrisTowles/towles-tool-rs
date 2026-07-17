// Auto-bumps a Claude Code plugin's `.claude-plugin/plugin.json` patch
// version whenever a commit touches that plugin's directory. Skipped for a
// plugin whose manifest version was already hand-edited in the same commit
// (e.g. a deliberate minor/major bump) — see `manifestsToBump`. Invoked by
// `.githooks/pre-commit`; wired up via the root "prepare" npm script running
// `git config core.hooksPath .githooks`.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

export const PLUGINS = [
  { dir: "packages/core", manifest: "packages/core/.claude-plugin/plugin.json" },
  { dir: "packages/app", manifest: "packages/app/.claude-plugin/plugin.json" },
];

/** Bumps the patch component of a `major.minor.patch` version string. */
export function nextPatchVersion(version) {
  const parts = version.split(".").map(Number);
  if (parts.length !== 3 || parts.some((n) => !Number.isInteger(n))) {
    throw new Error(`plugin.json version "${version}" is not major.minor.patch`);
  }
  parts[2] += 1;
  return parts.join(".");
}

/**
 * Which plugins need an auto-bump, given the set of staged file paths and a
 * `readVersions(manifest) -> { head, index }` lookup (HEAD's committed
 * version vs. the version currently staged in the index). Pure aside from
 * that injected reader, so it's testable without a real git repo.
 *
 * A plugin is skipped when: none of its files are staged, it has no HEAD
 * version yet (brand-new plugin — let the authored version stand), or its
 * manifest version already differs from HEAD (hand-edited this commit).
 */
export function manifestsToBump(stagedFiles, plugins, readVersions) {
  return plugins.filter((p) => {
    const touched = stagedFiles.some((f) => f === p.manifest || f.startsWith(`${p.dir}/`));
    if (!touched) return false;
    const { head, index } = readVersions(p.manifest);
    return head !== null && head === index;
  });
}

/** Rewrites just the `"version": "..."` line in-place, preserving all other formatting. */
export function withBumpedVersion(manifestContents, from, to) {
  const needle = `"version": "${from}"`;
  if (!manifestContents.includes(needle)) {
    throw new Error(`could not find ${needle} to replace`);
  }
  return manifestContents.replace(needle, `"version": "${to}"`);
}

function git(args) {
  return execFileSync("git", args, { encoding: "utf8" });
}

function manifestVersionAt(ref, manifest) {
  try {
    return JSON.parse(git(["show", `${ref}:${manifest}`])).version ?? null;
  } catch {
    return null;
  }
}

function stagedFiles() {
  return git(["diff", "--cached", "--name-only", "--diff-filter=ACMR"])
    .split("\n")
    .filter(Boolean);
}

/** Runs the real pre-commit bump against the current git index; used by `.githooks/pre-commit`. */
export function runPreCommitBump() {
  const toBump = manifestsToBump(stagedFiles(), PLUGINS, (manifest) => ({
    head: manifestVersionAt("HEAD", manifest),
    index: manifestVersionAt("", manifest),
  }));

  for (const { manifest } of toBump) {
    const contents = readFileSync(manifest, "utf8");
    const from = JSON.parse(contents).version;
    const to = nextPatchVersion(from);
    writeFileSync(manifest, withBumpedVersion(contents, from, to));
    git(["add", manifest]);
    console.log(`plugin-version-bump: ${manifest} ${from} -> ${to}`);
  }
}
