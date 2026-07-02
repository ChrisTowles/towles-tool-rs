// Phase-5 drop-in. NOT wired up yet — the Rust bridge (Tauri commands + the
// `agentboard://state` event) does not exist. This file exists to prove the
// `StateSource` / `Commands` interfaces fit a real Tauri backend without any
// change to the UI. `App` selects the mock while `__TAURI_INTERNALS__` is
// absent; swap in these classes once the bridge lands.

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

  async start(): Promise<void> {
    const { listen } = await import("@tauri-apps/api/event");
    const { invoke } = await import("@tauri-apps/api/core");
    this.unlisten = await listen<AgentBoardState>(STATE_EVENT, (e) => {
      this.latest = e.payload;
      for (const l of this.listeners) l(e.payload);
    });
    // Ask the bridge to push the current snapshot immediately.
    await invoke("ab_refresh");
  }

  stop(): void {
    this.unlisten?.();
    this.unlisten = null;
  }
}

export class TauriCommands implements Commands {
  private async call(cmd: string, args?: Record<string, unknown>): Promise<void> {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke(cmd, args);
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
  removeRepo(name: string): void {
    void this.call("ab_remove_repo", { name });
  }
  refresh(): void {
    void this.call("ab_refresh");
  }
}
