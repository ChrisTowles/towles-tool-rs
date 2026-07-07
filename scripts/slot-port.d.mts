// Hand-written declarations for slot-port.mjs so TypeScript consumers
// (apps/client/vite.config.ts, wdio.conf.ts) type-check under `tsc -b`.
// Keep in sync with the exports in slot-port.mjs.

export const PORT_MIN: number;

/** Stable base port for the slot rooted at `repoRoot` (keyed on its dir name). */
export function slotBasePort(repoRoot: string): number;

/**
 * Load `.env.local` (at `repoRoot`) into `process.env`. Real env vars win over
 * the file; a missing file is a no-op.
 */
export function loadEnvLocal(repoRoot: string): void;

/**
 * Resolve the dev-server port after loading `.env.local`; `null` for an
 * invalid explicit `TT_DEV_PORT`.
 */
export function resolveDevPort(repoRoot: string): number | null;

/** The embedded WebDriver server's port for a given dev port. */
export function resolveWebdriverPort(devPort: number): number;
