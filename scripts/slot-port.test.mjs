// Tests for the per-slot port derivation in `slot-port.mjs`, the single source
// of truth that dev / dev:drive / e2e / wdio.conf.ts all depend on. Run with
// `node --test scripts/` (built-in runner, no extra deps).
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  PORT_MIN,
  slotBasePort,
  loadEnvLocal,
  resolveDevPort,
  resolveWebdriverPort,
} from "./slot-port.mjs";

const PORT_SPAN = 200; // mirrors the private span in slot-port.mjs

// Run `fn` with `process.env` restored afterwards, so env-reading functions
// (resolveDevPort/resolveWebdriverPort) don't leak state between tests.
function withCleanEnv(keys, fn) {
  const saved = {};
  for (const key of keys) saved[key] = process.env[key];
  for (const key of keys) delete process.env[key];
  try {
    fn();
  } finally {
    for (const key of keys) {
      if (saved[key] === undefined) delete process.env[key];
      else process.env[key] = saved[key];
    }
  }
}

// Make a throwaway repo root, optionally seeding its `.env.local`.
function makeRepoRoot(envLocal) {
  const root = mkdtempSync(join(tmpdir(), "slot-port-test-"));
  if (envLocal !== undefined) writeFileSync(join(root, ".env.local"), envLocal);
  return root;
}

test("slotBasePort is stable for a given slot name", () => {
  const root = "/home/x/code/towles-tool-rs-slot-3";
  assert.equal(slotBasePort(root), slotBasePort(root));
  // Keyed only on the basename, not the full path.
  assert.equal(slotBasePort(root), slotBasePort("/other/prefix/towles-tool-rs-slot-3"));
});

test("slotBasePort stays within the partitioned range", () => {
  for (const name of ["slot-0", "slot-1", "towles-tool-rs-slot-7", "a", "", "zzzzzzzzzz"]) {
    const port = slotBasePort(`/root/${name}`);
    assert.ok(port >= PORT_MIN && port < PORT_MIN + PORT_SPAN, `${name} -> ${port}`);
  }
});

test("slotBasePort separates distinct slot names", () => {
  const a = slotBasePort("/root/towles-tool-rs-slot-0");
  const b = slotBasePort("/root/towles-tool-rs-slot-1");
  assert.notEqual(a, b);
});

test("resolveDevPort falls back to the slot base port with no override", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot();
    try {
      assert.equal(resolveDevPort(root), slotBasePort(root));
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveDevPort: shell TT_DEV_PORT wins over the base port", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot();
    try {
      process.env.TT_DEV_PORT = "5555";
      assert.equal(resolveDevPort(root), 5555);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveDevPort: .env.local pins the port when the shell env is unset", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot("TT_DEV_PORT=4321\n");
    try {
      assert.equal(resolveDevPort(root), 4321);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveDevPort: shell env wins over .env.local", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot("TT_DEV_PORT=4321\n");
    try {
      process.env.TT_DEV_PORT = "6000";
      assert.equal(resolveDevPort(root), 6000);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveDevPort returns null for an invalid TT_DEV_PORT", () => {
  for (const bad of ["0", "-1", "70000", "abc", "12.5"]) {
    withCleanEnv(["TT_DEV_PORT"], () => {
      const root = makeRepoRoot();
      try {
        process.env.TT_DEV_PORT = bad;
        assert.equal(resolveDevPort(root), null, `${bad} should be invalid`);
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    });
  }
});

test("resolveDevPort ignores an empty TT_DEV_PORT and uses the base port", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot();
    try {
      process.env.TT_DEV_PORT = "";
      assert.equal(resolveDevPort(root), slotBasePort(root));
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvLocal strips matching double and single quotes", () => {
  withCleanEnv(["TT_QUOTED_D", "TT_QUOTED_S", "TT_BARE"], () => {
    const root = makeRepoRoot('TT_QUOTED_D="dq"\nTT_QUOTED_S=\'sq\'\nTT_BARE=bare\n');
    try {
      loadEnvLocal(root);
      assert.equal(process.env.TT_QUOTED_D, "dq");
      assert.equal(process.env.TT_QUOTED_S, "sq");
      assert.equal(process.env.TT_BARE, "bare");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvLocal does not override an existing real env var", () => {
  withCleanEnv(["TT_EXISTING"], () => {
    const root = makeRepoRoot("TT_EXISTING=fromfile\n");
    try {
      process.env.TT_EXISTING = "fromshell";
      loadEnvLocal(root);
      assert.equal(process.env.TT_EXISTING, "fromshell");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvLocal is a no-op when .env.local is missing", () => {
  withCleanEnv(["TT_ABSENT"], () => {
    const root = makeRepoRoot();
    try {
      assert.doesNotThrow(() => loadEnvLocal(root));
      assert.equal(process.env.TT_ABSENT, undefined);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveWebdriverPort defaults to devPort + 3000", () => {
  withCleanEnv(["TT_E2E_WEBDRIVER_PORT"], () => {
    assert.equal(resolveWebdriverPort(1500), 4500);
  });
});

test("resolveWebdriverPort: TT_E2E_WEBDRIVER_PORT overrides the offset", () => {
  withCleanEnv(["TT_E2E_WEBDRIVER_PORT"], () => {
    process.env.TT_E2E_WEBDRIVER_PORT = "9999";
    assert.equal(resolveWebdriverPort(1500), 9999);
  });
});
