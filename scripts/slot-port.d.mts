// Hand-written declarations for slot-port.mjs so TypeScript importers
// (apps/client/vite.config.ts, e2e/wdio.conf.ts) typecheck. Keep in sync with
// the .mjs exports.

export declare const PORT_MIN: number;

/** Deterministic per-slot base port derived from the repo-root dir name. */
export declare function slotBasePort(repoRoot: string): number;

/** Read KEY=VALUE pairs from the repo's gitignored `.env.local`, if present. */
export declare function loadEnvLocal(repoRoot: string): Record<string, string>;

/** The slot's dev-server port: `TT_DEV_PORT` override or the slot base. */
export declare function resolveDevPort(repoRoot: string): number;

/** The in-app WebDriver server port paired with a dev port. */
export declare function resolveWebdriverPort(devPort: number): number;
