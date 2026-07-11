import { invokeToast } from "@/lib/tauri";

/**
 * Client-side bridge to the Claude Sessions screen (the Rust `tt-graph` crate,
 * surfaced via `crates-tauri/tt-app/src/claude_sessions.rs`). Plain
 * request/response, no live event stream.
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

export type ClaudeSession = {
  sessionId: string;
  title: string | null;
  project: string;
  date: string;
  tokens: number;
  mtime: number;
  /** Real absolute working directory the session ran in, or `null` for older
   * transcripts logged before Claude Code recorded it (no fork target then). */
  cwd: string | null;
};

export const claudeSessionsSummary = (days: number) =>
  invokeToast<SpendSummary>("claude_sessions_summary", { days });

export const claudeSessionsList = (days: number) =>
  invokeToast<ClaudeSession[]>("claude_sessions_list", { days });
