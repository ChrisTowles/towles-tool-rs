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

/**
 * `@wdio/tauri-plugin`'s init unconditionally logs two warnings on every
 * boot, and neither describes an app problem: the `TEST:` line is the
 * plugin's own console-forwarding self-check (fired deliberately to verify
 * the pipe works), and the defineProperty line announces an *expected*
 * fallback for a feature (invoke mocking via `window.__wdio_mocks__`) this
 * repo's drive/e2e harness never uses. Buffering them made every
 * `drive.mjs` verb report warnings that had to be eyeballed past, which is
 * exactly the alert-fatigue this collector exists to prevent — so they're
 * excluded from the buffer (they still reach the real console untouched).
 * Matched on their distinctive middles, not a blanket plugin-prefix filter:
 * a real plugin failure logs different text and must still land here.
 *
 * The `listeners[eventId].handlerId` rejection is Tauri core's, not ours:
 * the *injected* unlisten script (tauri `src/event/mod.rs`) guards the
 * per-event map but not the individual entry, so an unlisten racing a
 * just-resolved listen — the standard `if (disposed) sub()` unmount
 * pattern, exercised by boot-time pane churn — throws inside Tauri's own
 * script. Delivery is governed by the Rust-side registry (already cleaned
 * up by then), so the failure is cosmetic. Matched kind-aware and anchored:
 * only an unhandled *rejection* whose reason IS that TypeError is dropped —
 * an app-side `console.error("cleanup failed", e)` that merely mentions the
 * fragment still lands in the buffer. Upstream context:
 * tauri-apps/tauri#8916.
 */
const KNOWN_BENIGN_WARNS = [
  "TEST: This is a test WARN log after setupConsoleForwarding()",
  "Invoke interception via defineProperty failed",
] as const;

const TAURI_UNLISTEN_RACE =
  /^TypeError: undefined is not an object \(evaluating 'listeners\[eventId\]\.handlerId'\)/;

export function isKnownBenignEntry(kind: CapturedConsoleEntry["kind"], text: string): boolean {
  if (kind === "warn") return KNOWN_BENIGN_WARNS.some((needle) => text.includes(needle));
  if (kind === "rejection") return TAURI_UNLISTEN_RACE.test(text);
  return false;
}

function render(value: unknown): string {
  if (typeof value === "string") return value;
  if (value instanceof Error) {
    // The first stack frames ride along — a buffered rejection with no
    // provenance can't be triaged, only eyeballed past.
    const frames = value.stack?.split("\n").slice(0, 4).join(" ← ");
    return frames
      ? `${value.name}: ${value.message} [${frames}]`
      : `${value.name}: ${value.message}`;
  }
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
    if (isKnownBenignEntry(kind, text)) return;
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
