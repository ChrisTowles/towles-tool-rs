// The real bridge (committed 99db77e). `App` selects these classes when
// `__TAURI_INTERNALS__` is present; the mock stays in a bare browser so
// `npm run client:dev` remains a living demo. The bridge registers the `ab_*`
// commands and emits state on the `agentboard://state` event.

import type { AgentBoardState, ReorderDelta } from "../types";
import type { Commands, StateSource } from "./StateSource";

const STATE_EVENT = "agentboard://state";

export class TauriStateSource implements StateSource {
  private listeners = new Set<(s: AgentBoardState) => void>();
  private unlisten: (() => void) | null = null;
  private latest: AgentBoardState | null = null;

  subscribe(listener: (s: AgentBoardState) => void): () => void {
    this.listeners.add(listener);
    if (this.latest) listener(this.latest);
    return () => this.listeners.delete(listener);
  }

  private publish(state: AgentBoardState): void {
    this.latest = state;
    for (const l of this.listeners) l(state);
  }

  async start(): Promise<void> {
    const { listen } = await import("@tauri-apps/api/event");
    const { invoke } = await import("@tauri-apps/api/core");

    this.unlisten = await listen<AgentBoardState>(STATE_EVENT, (e) => this.publish(e.payload));

    // Seed synchronously so the list isn't empty before the first event.
    // `ab_get_state` returns the current StatePayload directly.
    try {
      const initial = await invoke<AgentBoardState>("ab_get_state");
      this.publish(initial);
    } catch {
      // Fall back to a push; ignore if that fails too (an event will follow).
    }
    // Belt-and-braces: ask the bridge to (re)broadcast the current snapshot.
    await invoke("ab_refresh").catch(() => {});
  }

  stop(): void {
    this.unlisten?.();
    this.unlisten = null;
  }
}

export class TauriCommands implements Commands {
  private errorListeners = new Set<(m: string) => void>();

  onError(listener: (m: string) => void): () => void {
    this.errorListeners.add(listener);
    return () => this.errorListeners.delete(listener);
  }

  private async call(cmd: string, args?: Record<string, unknown>): Promise<void> {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke(cmd, args);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      for (const l of this.errorListeners) l(message);
    }
  }

  markSeen(name: string): void {
    void this.call("ab_mark_seen", { name });
  }
  dismissAgent(session: string, agent: string, threadId?: string): void {
    void this.call("ab_dismiss_agent", { session, agent, threadId });
  }
  reorderSession(name: string, delta: ReorderDelta): void {
    void this.call("ab_reorder_session", { name, delta });
  }
  setTheme(theme: string): void {
    void this.call("ab_set_theme", { theme });
  }
  addRepo(path: string): void {
    void this.call("ab_add_repo", { path });
  }
  removeRepo(name: string): void {
    void this.call("ab_remove_repo", { name });
  }
  refresh(): void {
    void this.call("ab_refresh");
  }
}
