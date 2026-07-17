// Tests for the pure logic in `plugin-version-bump.mjs` (git IO is injected,
// so no real repo is needed). Run with `node --test scripts/`.
import { test } from "node:test";
import assert from "node:assert/strict";

import { nextPatchVersion, manifestsToBump, withBumpedVersion } from "./plugin-version-bump.mjs";

const PLUGINS = [
  { dir: "packages/core", manifest: "packages/core/.claude-plugin/plugin.json" },
  { dir: "packages/app", manifest: "packages/app/.claude-plugin/plugin.json" },
];

test("nextPatchVersion increments the patch component", () => {
  assert.equal(nextPatchVersion("0.0.159"), "0.0.160");
  assert.equal(nextPatchVersion("1.2.9"), "1.2.10");
});

test("nextPatchVersion rejects non major.minor.patch strings", () => {
  assert.throws(() => nextPatchVersion("1.2"));
  assert.throws(() => nextPatchVersion("1.2.x"));
});

test("manifestsToBump bumps a plugin whose files changed and version is untouched", () => {
  const staged = ["packages/core/commands/foo.md"];
  const result = manifestsToBump(staged, PLUGINS, () => ({ head: "0.0.159", index: "0.0.159" }));
  assert.deepEqual(result.map((p) => p.dir), ["packages/core"]);
});

test("manifestsToBump skips a plugin with no staged files", () => {
  const staged = ["packages/app/hooks/scripts/gh-pr-nudge.sh"];
  const result = manifestsToBump(staged, PLUGINS, () => ({ head: "0.0.159", index: "0.0.159" }));
  assert.deepEqual(result.map((p) => p.dir), ["packages/app"]);
});

test("manifestsToBump skips a plugin whose version was already hand-edited", () => {
  const staged = ["packages/core/commands/foo.md", "packages/core/.claude-plugin/plugin.json"];
  const result = manifestsToBump(staged, PLUGINS, () => ({ head: "0.0.159", index: "0.1.0" }));
  assert.deepEqual(result, []);
});

test("manifestsToBump skips a brand-new plugin with no HEAD version", () => {
  const staged = ["packages/core/commands/foo.md"];
  const result = manifestsToBump(staged, PLUGINS, () => ({ head: null, index: "0.0.1" }));
  assert.deepEqual(result, []);
});

test("manifestsToBump matches on the manifest path itself", () => {
  const staged = ["packages/core/.claude-plugin/plugin.json"];
  const result = manifestsToBump(staged, PLUGINS, () => ({ head: "0.0.159", index: "0.0.159" }));
  assert.deepEqual(result.map((p) => p.dir), ["packages/core"]);
});

test("withBumpedVersion rewrites only the version line", () => {
  const contents = '{\n  "name": "tt",\n  "version": "0.0.159",\n  "author": {}\n}\n';
  const updated = withBumpedVersion(contents, "0.0.159", "0.0.160");
  assert.equal(updated, '{\n  "name": "tt",\n  "version": "0.0.160",\n  "author": {}\n}\n');
});

test("withBumpedVersion throws when the expected version line is missing", () => {
  const contents = '{\n  "version": "0.0.159"\n}\n';
  assert.throws(() => withBumpedVersion(contents, "9.9.9", "9.9.10"));
});
