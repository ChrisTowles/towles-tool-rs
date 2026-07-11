import { invokeToast } from "@/lib/tauri";

/**
 * Client-side bridge to the Claude Sessions screen (the Rust `tt-graph`
 * ledger module, surfaced via `crates-tauri/tt-app/src/claude_sessions.rs`).
 * Plain request/response; the backend caches the scan so search stays
 * in-memory.
 */

export type ProjectBar = {
  project: string;
  totalTokens: number;
};

export type ModelBar = {
  model: string;
  totalTokens: number;
};

export type LedgerDay = {
  date: string;
  projects: ProjectBar[];
};

export type LedgerTotals = {
  sessions: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
};

export type ClaudeSession = {
  sessionId: string;
  title: string | null;
  project: string;
  date: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  /** Real launch directory, for "Open in Agentboard"; null for transcripts
   * that predate the field. */
  cwd: string | null;
  /** Prompt-text context around the match; only present on search hits. */
  snippet?: string;
};

export type ClaudeSessionsSummary = {
  totals: LedgerTotals;
  days: LedgerDay[];
  byProject: ProjectBar[];
  byModel: ModelBar[];
  topSessions: ClaudeSession[];
};

export const claudeSessionsSummary = (days: number) =>
  invokeToast<ClaudeSessionsSummary>("claude_sessions_summary", { days });

export const claudeSessionsSearch = (days: number, query: string) =>
  invokeToast<ClaudeSession[]>("claude_sessions_search", { days, query });
