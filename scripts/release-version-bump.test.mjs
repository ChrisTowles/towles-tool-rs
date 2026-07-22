// Tests for the pure logic in `release-version-bump.mjs` (no filesystem or
// git IO — `main`'s side effects aren't exercised here). Run with
// `node --test scripts/`.
import { test } from "node:test";
import assert from "node:assert/strict";

import { BadVersion, VersionLineMissing } from "./errors.mjs";
import { parseVersion, resolveNewVersion, withBumpedVersion } from "./release-version-bump.mjs";

test("parseVersion accepts major.minor.patch", () => {
  assert.deepEqual(parseVersion("0.1.1").unwrap(), [0, 1, 1]);
  assert.deepEqual(parseVersion("12.3.400").unwrap(), [12, 3, 400]);
});

test("parseVersion errs BadVersion on malformed input", () => {
  for (const bad of ["1.2", "1.2.x", "1.2.3.4", "", "-1.0.0"]) {
    const result = parseVersion(bad);
    assert.ok(result.isErr(), `${bad} should be rejected`);
    assert.ok(BadVersion.is(result.error));
    assert.equal(result.error.version, bad);
  }
});

test("resolveNewVersion bumps major/minor/patch off the current version", () => {
  assert.equal(resolveNewVersion("0.1.1", "major").unwrap(), "1.0.0");
  assert.equal(resolveNewVersion("0.1.1", "minor").unwrap(), "0.2.0");
  assert.equal(resolveNewVersion("0.1.1", "patch").unwrap(), "0.1.2");
});

test("resolveNewVersion accepts an explicit x.y.z", () => {
  assert.equal(resolveNewVersion("0.1.1", "2.0.0").unwrap(), "2.0.0");
});

test("resolveNewVersion errs BadVersion for neither a keyword nor a valid version", () => {
  const result = resolveNewVersion("0.1.1", "next");
  assert.ok(result.isErr());
  assert.ok(BadVersion.is(result.error));
});

test("resolveNewVersion errs BadVersion when the current version itself is malformed", () => {
  const result = resolveNewVersion("not-a-version", "patch");
  assert.ok(result.isErr());
  assert.ok(BadVersion.is(result.error));
});

test("withBumpedVersion rewrites a JSON manifest's version line", () => {
  const contents = '{\n  "name": "tt-app",\n  "version": "0.1.1"\n}\n';
  const updated = withBumpedVersion(contents, "json", "0.1.1", "0.2.0").unwrap();
  assert.equal(updated, '{\n  "name": "tt-app",\n  "version": "0.2.0"\n}\n');
});

test("withBumpedVersion rewrites a Cargo.toml version line", () => {
  const contents = '[package]\nname = "tt-app"\nversion = "0.1.1"\nedition = "2024"\n';
  const updated = withBumpedVersion(contents, "toml", "0.1.1", "0.2.0").unwrap();
  assert.equal(updated, '[package]\nname = "tt-app"\nversion = "0.2.0"\nedition = "2024"\n');
});

test("withBumpedVersion only replaces the first match, leaving other same-version text alone", () => {
  // Guards the exact scenario this script was written to avoid: a naive
  // global replace would also corrupt an unrelated crate pinned to the same
  // version elsewhere in the file (e.g. a dependency entry in Cargo.lock).
  const contents = 'version = "0.1.1"\nother = "0.1.1"\n';
  const updated = withBumpedVersion(contents, "toml", "0.1.1", "0.2.0").unwrap();
  assert.equal(updated, 'version = "0.2.0"\nother = "0.1.1"\n');
});

test("withBumpedVersion errs VersionLineMissing when the expected line is absent", () => {
  const contents = '{\n  "version": "0.1.1"\n}\n';
  const result = withBumpedVersion(contents, "json", "9.9.9", "9.9.10");
  assert.ok(result.isErr());
  assert.ok(VersionLineMissing.is(result.error));
  assert.equal(result.error.needle, '"version": "9.9.9"');
});
