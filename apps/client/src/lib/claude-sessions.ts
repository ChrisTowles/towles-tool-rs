import { invokeToast } from "@/lib/tauri";

/**
 * Client-side bridge to the Claude Sessions screen (the Rust
 * `tt-claude-sessions` crate, surfaced via
 * `crates-tauri/tt-app/src/claude_sessions.rs`). Plain request/response; the
 * backend caches the scan so search stays in-memory.
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

export type InsightKind = "tokenOutlier" | "rereadLoop" | "cacheChurn" | "marathon";

/** One ranked waste finding with its session attached. */
export type ClaudeSessionInsight = {
  kind: InsightKind;
  /** Headline number, e.g. "6.2× median" or "38 re-reads". */
  metric: string;
  /** One-sentence "why this matters". */
  detail: string;
  session: ClaudeSession;
};

/** Ranked waste/habit findings for the window (rides the cached scan). */
export const claudeSessionsInsights = (days: number) =>
  invokeToast<ClaudeSessionInsight[]>("claude_sessions_insights", { days });

export type ToolTotal = {
  name: string;
  /** Call count as "Nx". */
  detail?: string;
  inputTokens: number;
  outputTokens: number;
};

export type TurnBreakdown = {
  name: string;
  inputTokens: number;
  outputTokens: number;
  /** Dominant tool for color-coding; null for user prompts. */
  toolName: string | null;
  model: string;
};

export type SessionBreakdown = {
  /** Tools ranked by attributed tokens. */
  tools: ToolTotal[];
  /** Session steps in transcript order. */
  turns: TurnBreakdown[];
};

/** One session's turn/tool drill-down (parses that session on demand). */
export const claudeSessionsBreakdown = (sessionId: string) =>
  invokeToast<SessionBreakdown>("claude_sessions_breakdown", { sessionId });
