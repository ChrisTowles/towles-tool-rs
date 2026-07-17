// Hand-written declarations for slot-port.mjs so TypeScript consumers
// (apps/client/vite.config.ts, wdio.conf.ts) type-check under `tsc -b`.
// Keep in sync with the exports in slot-port.mjs.

/**
 * Load `.env.local` then `.env` (at `repoRoot`) into `process.env`. Real env
 * vars win over the files; missing files are a no-op.
 */
export function loadEnvFiles(repoRoot: string): void;

/**
 * Resolve the dev-server port after loading the env files; `null` when
 * `TT_DEV_PORT` is unset or invalid.
 */
export function resolveDevPort(repoRoot: string): number | null;

/** The `tt slot env` name for the checkout at `repoRoot` (`primary` or the slot dir name). */
export function slotEnvName(repoRoot: string): string;

/**
 * Resolve the dev port or exit(1): invalid TT_DEV_PORT errors; unset with
 * `render` runs `tt slot env` to claim ports first.
 */
export function requireDevPort(
  repoRoot: string,
  opts?: { tag?: string; render?: boolean },
): number;

/** The embedded WebDriver server's port for a given dev port. */
export function resolveWebdriverPort(devPort: number): number;

/** Whether `port` is free to bind on both IPv4 and IPv6 loopback. */
export function isPortFree(port: number): Promise<boolean>;

/** Kill whatever's already listening on `port` (whole process group). */
export function killPort(port: number): Promise<void>;
