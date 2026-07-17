// Tests for the per-slot port derivation in `slot-port.mjs`, the single source
// of truth that dev / dev:drive / e2e / wdio.conf.ts all depend on. Run with
// `node --test scripts/` (built-in runner, no extra deps).
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn, execFileSync } from "node:child_process";

import {
  loadEnvFiles,
  resolveDevPort,
  slotEnvName,
  resolveWebdriverPort,
  isPortFree,
  killPort,
} from "./slot-port.mjs";

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

test("slotEnvName maps a nested worktree slot to its dir name", () => {
  assert.equal(slotEnvName("/home/x/code/blog/.claude/worktrees/feat-thing"), "feat-thing");
});

test("slotEnvName maps any other checkout to primary", () => {
  assert.equal(slotEnvName("/home/x/code/blog"), "primary");
  assert.equal(slotEnvName("/home/x/code/blog/.claude/other/feat-thing"), "primary");
});

test("resolveDevPort is null with no TT_DEV_PORT anywhere (no derived fallback)", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot();
    try {
      assert.equal(resolveDevPort(root), null);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveDevPort: shell TT_DEV_PORT wins", () => {
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

test("resolveDevPort treats an empty TT_DEV_PORT as unset", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot();
    try {
      process.env.TT_DEV_PORT = "";
      assert.equal(resolveDevPort(root), null);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvFiles strips matching double and single quotes", () => {
  withCleanEnv(["TT_QUOTED_D", "TT_QUOTED_S", "TT_BARE"], () => {
    const root = makeRepoRoot('TT_QUOTED_D="dq"\nTT_QUOTED_S=\'sq\'\nTT_BARE=bare\n');
    try {
      loadEnvFiles(root);
      assert.equal(process.env.TT_QUOTED_D, "dq");
      assert.equal(process.env.TT_QUOTED_S, "sq");
      assert.equal(process.env.TT_BARE, "bare");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvFiles does not override an existing real env var", () => {
  withCleanEnv(["TT_EXISTING"], () => {
    const root = makeRepoRoot("TT_EXISTING=fromfile\n");
    try {
      process.env.TT_EXISTING = "fromshell";
      loadEnvFiles(root);
      assert.equal(process.env.TT_EXISTING, "fromshell");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvFiles is a no-op when .env.local is missing", () => {
  withCleanEnv(["TT_ABSENT"], () => {
    const root = makeRepoRoot();
    try {
      assert.doesNotThrow(() => loadEnvFiles(root));
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

// --- .env layering (rendered by `tt slot`) ---

test("resolveDevPort: rendered .env pins the port when .env.local is absent", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot();
    writeFileSync(join(root, ".env"), "TT_DEV_PORT=1505\n");
    try {
      assert.equal(resolveDevPort(root), 1505);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("resolveDevPort: .env.local pin beats the rendered .env claim", () => {
  withCleanEnv(["TT_DEV_PORT"], () => {
    const root = makeRepoRoot("TT_DEV_PORT=4321\n");
    writeFileSync(join(root, ".env"), "TT_DEV_PORT=1505\n");
    try {
      assert.equal(resolveDevPort(root), 4321);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

test("loadEnvFiles: keys merge across both files, first seen wins per key", () => {
  withCleanEnv(["TT_DEV_PORT", "TT_ONLY_ENV"], () => {
    const root = makeRepoRoot("TT_DEV_PORT=4321\n");
    writeFileSync(join(root, ".env"), "TT_DEV_PORT=1505\nTT_ONLY_ENV=yes\n");
    try {
      loadEnvFiles(root);
      assert.equal(process.env.TT_DEV_PORT, "4321");
      assert.equal(process.env.TT_ONLY_ENV, "yes");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

// --- killPort ---

async function findEphemeralFreePort() {
  const { createServer } = await import("node:net");
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

test(
  "killPort stops a detached listener and frees the port",
  { skip: process.platform === "win32" },
  async () => {
    const port = await findEphemeralFreePort();
    const child = spawn(
      process.execPath,
      ["-e", `require("node:net").createServer((s) => s.destroy()).listen(${port}, "127.0.0.1")`],
      { stdio: "ignore", detached: true },
    );
    const pid = child.pid;
    child.unref();

    // Wait for it to actually bind before asserting it's up.
    for (let i = 0; i < 50 && (await isPortFree(port)); i++) {
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    assert.equal(await isPortFree(port), false, "child should be bound to the port");

    await killPort(port);

    assert.equal(await isPortFree(port), true, "port should be free after killPort");
    assert.throws(
      () => execFileSync("ps", ["-p", String(pid)], { stdio: "ignore" }),
      "child process should no longer exist",
    );
  },
);

test("killPort is a no-op when nothing is listening", { skip: process.platform === "win32" }, async () => {
  const port = await findEphemeralFreePort();
  await assert.doesNotReject(() => killPort(port));
});
