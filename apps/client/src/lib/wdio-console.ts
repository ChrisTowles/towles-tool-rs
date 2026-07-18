/**
 * E2E/live-drive only: buffer console errors, warnings and uncaught exceptions
 * on `window` so the out-of-process harness can read them back. See the
 * testing section of apps/client/CLAUDE.md for why this is the only signal
 * available for a runtime React complaint.
 *
 * Installed synchronously before `createRoot`, deliberately: warnings fired
 * during the first render are exactly the ones worth catching, and a dynamic
 * import would resolve too late to see them.
 */

/** Where the buffer lives on `window`. Cross-process contract with
 * `scripts/drive.mjs`'s `CONSOLE_KEY` — renaming one side silently disables
 * the check rather than failing. */
export const WDIO_CONSOLE_KEY = "__ttConsoleErrors";

/** Bounded so a render loop screaming into console.error can't exhaust memory. */
const MAX_ENTRIES = 200;

export type CapturedConsoleEntry = {
  kind: "error" | "warn" | "exception" | "rejection";
  text: string;
  at: number;
};

function render(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) return `${value.name}: ${value.message}`;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    // Circular / exotic objects: the type is still a useful breadcrumb.
    return Object.prototype.toString.call(value);
  }
}

export function installConsoleCollector(): void {
  const win = window as unknown as Record<string, unknown>;
  // Idempotent — a hot reload must not stack wrappers around console.error.
  if (win[WDIO_CONSOLE_KEY]) return;

  const buffer: CapturedConsoleEntry[] = [];
  win[WDIO_CONSOLE_KEY] = buffer;

  const push = (kind: CapturedConsoleEntry["kind"], text: string) => {
    if (buffer.length >= MAX_ENTRIES) buffer.shift();
    buffer.push({ kind, text: text.slice(0, 2000), at: Date.now() });
  };

  for (const kind of ["error", "warn"] as const) {
    const original = console[kind].bind(console);
    console[kind] = (...args: unknown[]) => {
      push(kind, args.map(render).join(" "));
      // Still forward it — the dev:drive terminal stays as useful as before.
      original(...args);
    };
  }

  window.addEventListener("error", (e) => {
    push("exception", e.message ? `${e.message} (${e.filename}:${e.lineno})` : "unknown error");
  });
  window.addEventListener("unhandledrejection", (e) => {
    push("rejection", render(e.reason));
  });
}
