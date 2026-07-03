// Contract with the Rust bridge (see docs/AGENTBOARD-BRIDGE-SPEC.md §6).
//
// This is the TRIMMED state payload the desktop bridge will emit. Compared to
// the TS TUI's `SessionData`, the following server-only / tmux-vestigial fields
// are dropped: `panes`, `windows`, `uptime`, `createdAt`, `isWorktree`,
// `eventTimestamps`, `ports`, plus the payload extras `sidebarWidth`.

export type AgentStatus =
  | "idle"
  | "busy"
  | "complete"
  | "error"
  | "idle"
  | "waiting"
  | "interrupted";

/** Self-paced `/loop` state — the session scheduled its own next wake-up. */
export interface LoopInfo {
  /** Epoch ms when the loop is scheduled to fire next. In the past once ended. */
  nextWakeAt: number;
  /** Short reason the session gave for the scheduled wake-up. */
  reason?: string;
}

/** A sub-agent spawned by the parent session (workflow fan-out / background Task). */
export interface SubagentInfo {
  /** Sub-agent type, e.g. "Explore", "general-purpose", a workflow label. */
  agentType?: string;
  /** Short human description of the sub-agent's task. */
  description?: string;
}

/** Per-agent live details — populated by the claude-code watcher. */
export interface AgentDetails {
  /** Model name from the most recent assistant turn (e.g. "claude-opus-4-6"). */
  model?: string;
  /** Epoch ms when the prompt cache expires; undefined = no cache active. */
  cacheExpiresAt?: number;
  /** Cache TTL type in ms: 300_000 (5m) or 3_600_000 (1h). */
  cacheTtlMs?: number;
  /** Epoch ms of the most recent assistant entry in the journal. */
  lastActivityAt?: number;
  /** Name of the most recent tool invoked (e.g. "Read", "Bash", "Edit"). */
  lastTool?: string;
  /** Currently-active sub-agents. Empty/undefined when none are live. */
  subagents?: SubagentInfo[];
  /** Set when running a self-paced `/loop`. */
  loop?: LoopInfo;
}

/** A single agent instance shown as an AgentRow. */
export interface AgentDisplay {
  agent: string;
  session: string;
  status: AgentStatus;
  ts: number;
  threadId?: string;
  threadName?: string;
  /** True if the user hasn't seen this terminal state yet. */
  unseen?: boolean;
  details?: AgentDetails;
}

// --- Programmatic metadata (agent/script-pushed) ---

export type MetadataTone = "neutral" | "info" | "success" | "warn" | "error";

export interface MetadataStatus {
  text: string;
  tone?: MetadataTone;
  ts: number;
}

export interface MetadataProgress {
  current?: number;
  total?: number;
  percent?: number;
  label?: string;
  ts: number;
}

export interface MetadataLogEntry {
  message: string;
  tone?: MetadataTone;
  source?: string;
  ts: number;
}

export interface SessionMetadata {
  status: MetadataStatus | null;
  progress: MetadataProgress | null;
  logs: MetadataLogEntry[];
}

/** The trimmed per-session record. */
export interface SessionData {
  name: string;
  dir: string;
  branch: string;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  commitsDelta: number;
  unseen: boolean;
  /** Highest-priority agent, or null when the session has no agents. */
  agentState: AgentDisplay | null;
  agents: AgentDisplay[];
  metadata?: SessionMetadata | null;
}

/**
 * The full state snapshot the bridge emits on the `agentboard://state` event
 * and returns from `ab_get_state`. `preferredEditor` is carried for contract
 * fidelity but not yet consumed by the UI.
 */
export interface AgentBoardState {
  sessions: SessionData[];
  theme: string | undefined;
  preferredEditor?: string;
  ts: number;
}

/** Delta for reorder-session. */
export type ReorderDelta = "up" | "down" | "top" | "bottom";
