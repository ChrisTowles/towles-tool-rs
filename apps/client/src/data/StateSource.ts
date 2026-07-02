import type { AgentBoardState, ReorderDelta } from "../types";

/**
 * A source of AgentBoard state snapshots. The mock emits an evolving demo on a
 * timer; the Tauri source (phase 5) will listen to the `agentboard://state`
 * event. `subscribe` returns an unsubscribe fn and should replay the latest
 * snapshot synchronously to a new subscriber if one exists.
 */
export interface StateSource {
  subscribe(listener: (state: AgentBoardState) => void): () => void;
  start(): void;
  stop(): void;
}

/**
 * Client → bridge commands. The mock impl mutates the mock state in place; the
 * Tauri impl (phase 5) will `invoke` the matching Tauri commands. Names mirror
 * BRIDGE-SPEC §3 with the tmux routing dropped and kill/new-session replaced by
 * remove/add-repo.
 */
export interface Commands {
  /** Clear the unseen flag for a session (mark-seen). */
  markSeen(name: string): void;
  /** Remove a single agent instance (dismiss-agent). */
  dismissAgent(session: string, agent: string, threadId?: string): void;
  /** Move a session within the custom order (reorder-session). */
  reorderSession(name: string, delta: ReorderDelta): void;
  /** Persist + broadcast the theme (set-theme). */
  setTheme(theme: string): void;
  /** Remove a repo from config (was kill-session). */
  removeRepo(name: string): void;
  /** Force a rebuild/broadcast (refresh). */
  refresh(): void;
}
