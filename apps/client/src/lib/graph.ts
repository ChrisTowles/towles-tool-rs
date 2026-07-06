import { toast } from "sonner";
import { isTauri } from "@/lib/data";

/**
 * Client-side bridge to the Graph screen (the Rust `tt-graph` crate, surfaced via
 * `crates-tauri/tt-app/src/graph.rs`). Plain request/response, no live event stream.
 */

export type ProjectBar = {
  project: string;
  totalTokens: number;
};

export type ModelBar = {
  model: string;
  totalTokens: number;
};

export type SpendSummary = {
  byProject: ProjectBar[];
  byModel: ModelBar[];
};

async function graphInvoke<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T | null> {
  if (!isTauri()) return null;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    return await invoke<T>(command, args);
  } catch (e) {
    toast.error(String(e));
    return null;
  }
}

export const graphSpendSummary = (days: number) =>
  graphInvoke<SpendSummary>("graph_spend_summary", { days });
