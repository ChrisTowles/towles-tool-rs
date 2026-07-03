import type { AgentBoardState, ReorderDelta } from "../types";

/**
 * A source of AgentBoard state snapshots. The mock emits an evolving demo on a
 * timer; the Tauri source listens to the `agentboard://state` event (and seeds
 * from `ab_get_state` on start). `subscribe` returns an unsubscribe fn and
 * should replay the latest snapshot synchronously to a new subscriber if one
 * exists.
 */
export interface StateSource {
  subscribe(listener: (state: AgentBoardState) => void): () => void;
  start(): void;
  stop(): void;
}

/**
 * Client → bridge commands. The mock impl mutates the mock state in place; the
 * Tauri impl `invoke`s the matching `ab_*` commands. Names mirror BRIDGE-SPEC
 * §3 with the tmux routing dropped and kill/new-session replaced by
 * remove/add-repo. Every `ab_*` command returns a `Result`; a rejection is
 * delivered to `onError` subscribers (the mock never errors).
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
  /** Add a repo to the board by absolute path (ab_add_repo). */
  addRepo(path: string): void;
  /** Remove a repo from config (was kill-session). */
  removeRepo(name: string): void;
  /** Force a rebuild/broadcast (refresh). */
  refresh(): void;
  /**
   * Open the zellij terminal window (zellij_open). Resolves with a login
   * token only when the bridge had to create the first one — zellij shows a
   * token exactly once, so the UI must display it for the login form.
   */
  openZellij(): Promise<string | null>;
  /** Subscribe to command errors (a rejected invoke). Returns unsubscribe. */
  onError(listener: (message: string) => void): () => void;
}
