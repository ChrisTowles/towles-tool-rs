import { invoke } from "@/lib/tauri";

/**
 * Client-side bridge to the Telemetry screen (the Rust `tt-telemetry` crate,
 * surfaced via `crates-tauri/tt-app/src/telemetry.rs`). Reads the on-disk
 * event log fresh on every call — no server-side cache, unlike Claude
 * Sessions — since the log is small and the screen refreshes on a manual
 * button and on regaining focus rather than needing to survive rapid
 * re-renders.
 */

export type TelemetryRecord = {
  ts: string;
  /** `"event"` or `"span"`. */
  kind: string;
  level: string;
  target: string;
  name: string;
  /** The worktree/task scope that produced this record, if any. */
  ttTask: string | null;
  /** Present only on `kind: "span"` records. */
  durationMs: number | null;
  /** Every other field on the line. */
  fields: Record<string, unknown>;
  /** The original JSON line, verbatim. */
  raw: string;
};

/** Dates with a log file on disk, newest first. */
export const telemetryDays = () => invoke<string[]>("telemetry_days");

/** One day's records, in the order they were written. */
export const telemetryEvents = (date: string) =>
  invoke<TelemetryRecord[]>("telemetry_events", { date });

export const LEVELS = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] as const;
export type LevelFilter = "all" | (typeof LEVELS)[number];
export type KindFilter = "all" | "event" | "span";

/**
 * The Log tab's filter selections, persisted across screen switches and app
 * restarts (`tt-telemetry-filters`, the same localStorage idiom as the
 * workspace tab state). The day picker is deliberately *not* here — a stale
 * date is more confusing than useful, so it resets to the newest day each
 * visit.
 */
export type TelemetryFilters = {
  level: LevelFilter;
  kind: KindFilter;
  target: string;
  query: string;
};

export const DEFAULT_TELEMETRY_FILTERS: TelemetryFilters = {
  level: "all",
  kind: "all",
  target: "all",
  query: "",
};

export const TELEMETRY_FILTERS_KEY = "tt-telemetry-filters";

const LEVEL_VALUES = new Set<string>(["all", ...LEVELS]);
const KIND_VALUES = new Set<string>(["all", "event", "span"]);

/**
 * Restore persisted Log-tab filters from a raw localStorage string, degrading
 * any missing or malformed field to its default rather than throwing — a
 * corrupt value can never break the screen.
 *
 * Pure (callers pass the raw string) so it can be unit tested, mirroring
 * `loadWorkspaceTabs`. `target` is kept verbatim since valid targets are
 * data-dependent (they vary by day) and can't be validated against a fixed
 * set here; the screen falls it back to "all" when the loaded day has no such
 * target.
 */
export function loadTelemetryFilters(raw: string | null): TelemetryFilters {
  if (raw === null) return DEFAULT_TELEMETRY_FILTERS;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return DEFAULT_TELEMETRY_FILTERS;
  }
  if (typeof parsed !== "object" || parsed === null) return DEFAULT_TELEMETRY_FILTERS;
  const p = parsed as Record<string, unknown>;
  return {
    level: typeof p.level === "string" && LEVEL_VALUES.has(p.level) ? (p.level as LevelFilter) : "all",
    kind: typeof p.kind === "string" && KIND_VALUES.has(p.kind) ? (p.kind as KindFilter) : "all",
    target: typeof p.target === "string" ? p.target : "all",
    query: typeof p.query === "string" ? p.query : "",
  };
}

export function saveTelemetryFilters(filters: TelemetryFilters): void {
  localStorage.setItem(TELEMETRY_FILTERS_KEY, JSON.stringify(filters));
}
