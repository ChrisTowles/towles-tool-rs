import { invoke } from "@/lib/tauri";

/**
 * Client-side bridge to the Task Explorer screen (`crates-tauri/tt-app/src/
 * task_explorer.rs`): CPU/RAM for the app's own process plus each live
 * terminal's shell and everything it has spawned. Passive readout, polled
 * by the screen on an interval, and also polled (summed) by the status bar
 * (`components/status-bar.tsx`) for its always-visible total.
 */

export type ProcessRow = {
  pid: number;
  parentPid: number | null;
  name: string;
  /** Percent of the whole machine's CPU (all cores). */
  cpuPercent: number;
  memoryBytes: number;
  status: string;
};

export type ProcessGroup = {
  /** `null` for the app's own process group. */
  termId: string | null;
  label: string;
  rows: ProcessRow[];
  totalCpuPercent: number;
  totalMemoryBytes: number;
};

export const taskExplorerSnapshot = () => invoke<ProcessGroup[]>("task_explorer_snapshot");
