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
