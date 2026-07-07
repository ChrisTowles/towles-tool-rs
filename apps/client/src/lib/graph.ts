import { invokeToast } from "@/lib/tauri";

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

export const graphSpendSummary = (days: number) =>
  invokeToast<SpendSummary>("graph_spend_summary", { days });
