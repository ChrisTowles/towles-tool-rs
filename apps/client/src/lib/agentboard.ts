import { useEffect, useState } from "react";

/**
 * Client-side view of the agentboard bridge (`crates-tauri/tt-app/src/agentboard.rs`).
 * Mirrors the serialized `StatePayload` / `SessionData` (camelCase) that the
 * `ab_get_state` command returns and the `agentboard://state` event broadcasts.
 * Only the fields the screen renders are typed; the payload carries more.
 */

export type AgentStatus =
  "idle" | "busy" | "complete" | "error" | "waiting" | "interrupted";

export type AgentEvent = {
  agent: string;
  session: string;
  status: AgentStatus;
  ts: number;
  threadName?: string;
  unseen?: boolean;
};

/** One PTY shell inside a folder. "Agent" is a badge: `agentState` is set when
 * Claude (or another agent) is detected running in this PTY. */
export type SessionData = {
  id: string;
  name: string;
  createdAt: number;
  unseen: boolean;
  agentState?: AgentEvent | null;
  agents: AgentEvent[];
};

/** One checkout of a repo on disk (a clone, worktree, or slot). */
export type FolderData = {
  name: string;
  dir: string;
  branch: string;
  isWorktree: boolean;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  commitsDelta: number;
  sessions: SessionData[];
  needs: number;
};

/** A logical repo: the group of checkouts sharing a `git remote origin` URL. */
export type RepoData = {
  key: string;
  name: string;
  originUrl?: string | null;
  folders: FolderData[];
  needs: number;
};

export type StatePayload = {
  repos: RepoData[];
  theme?: string | null;
  preferredEditor: string;
  ts: number;
};

/** A session is an "agent" session iff Claude is running in it right now. */
export function isAgent(s: SessionData): boolean {
  return s.agentState != null;
}

/** A session "needs you" when its agent is blocked on input or errored. */
export function sessionNeeds(s: SessionData): boolean {
  return s.agentState?.status === "waiting" || s.agentState?.status === "error";
}

/** The label a session row leads with: when an agent is running, its task/thread
 * name (so the row reads as *the agent*, not a bare "shell 1"); otherwise the
 * shell's own name. The shell name stays available as a secondary tag. */
export function sessionLabel(s: SessionData): string {
  const thread = s.agentState?.threadName?.trim();
  return thread && thread.length > 0 ? thread : s.name;
}

/** The Claude session title carried by a terminal's OSC window title, or null
 * when it isn't a Claude one. Claude Code emits `✳ <session title>`; a plain
 * shell sets its own cwd/command title (no glyph). The generic `✳ Claude Code`
 * means "running, no session title yet" → null, so the caller falls back to
 * its other label (the /proc-derived task name or the shell name). */
export function claudeTitleName(raw: string | undefined): string | null {
  if (!raw) return null;
  const m = raw.match(/^\s*✳\s*(.+?)\s*$/u);
  if (!m || m[1] === "Claude Code") return null;
  return m[1];
}

/** A one-liner status message for a session row. */
export function sessionStatusText(s: SessionData): string {
  const st = s.agentState;
  if (!st) return "idle";
  switch (st.status) {
    case "waiting":
      return "Waiting — needs your input";
    case "error":
      return "Errored — needs a look";
    case "busy":
      return "Working…";
    case "complete":
      return "Done";
    case "interrupted":
      return "Paused";
    default:
      return "idle";
  }
}

/** True when a repo's single folder should collapse into one rail header. */
export function isSoloRepo(r: RepoData): boolean {
  return r.folders.length === 1;
}

const EMPTY: StatePayload = { repos: [], preferredEditor: "", ts: 0 };

/**
 * Subscribe to the live agentboard state: pull the initial snapshot via
 * `ab_get_state`, then track the debounced `agentboard://state` event. Returns
 * the latest payload (empty until the first snapshot arrives).
 */
export function useAgentboardState(): StatePayload {
  const [state, setState] = useState<StatePayload>(EMPTY);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      // Outside Tauri (bare-browser dev), `listen` throws on the missing IPC
      // internals — stay on the empty state instead of leaking unhandled
      // rejections.
      if (!("__TAURI_INTERNALS__" in window)) {
        setState(EMPTY);
        return;
      }

      const { invoke } = await import("@tauri-apps/api/core");
      const { listen } = await import("@tauri-apps/api/event");

      const sub = await listen<StatePayload>("agentboard://state", (e) => {
        setState(e.payload);
      });
      if (disposed) {
        sub();
        return;
      }
      unlisten = sub;

      try {
        const initial = await invoke<StatePayload>("ab_get_state");
        if (!disposed) setState(initial);
      } catch {
        // Bridge not ready — stay on EMPTY.
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return state;
}

/** Status dot color, mirroring the Rust `AgentStatus::color` intent. */
export function statusColor(status: AgentStatus): string {
  switch (status) {
    case "busy":
      return "bg-yellow-500";
    case "complete":
      return "bg-green-500";
    case "error":
      return "bg-red-500";
    case "waiting":
      return "bg-blue-500";
    case "interrupted":
      return "bg-orange-500";
    default:
      return "bg-muted-foreground/40";
  }
}
