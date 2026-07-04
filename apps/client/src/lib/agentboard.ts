import { useEffect, useState } from "react";

/**
 * Client-side view of the agentboard bridge (`crates-tauri/tt-app/src/agentboard.rs`).
 * Mirrors the serialized `StatePayload` / `SessionData` (camelCase) that the
 * `ab_get_state` command returns and the `agentboard://state` event broadcasts.
 * Only the fields the screen renders are typed; the payload carries more.
 */

export type AgentStatus =
  | "idle"
  | "busy"
  | "complete"
  | "error"
  | "waiting"
  | "interrupted";

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

/** Fake state for bare-browser dev (no Tauri), so the Folder Rail renders. */
const MOCK_STATE: StatePayload = {
  preferredEditor: "code",
  ts: 0,
  repos: [
    {
      key: "https://github.com/ChrisTowles/towles-tool-rs.git",
      name: "towles-tool-rs",
      originUrl: "https://github.com/ChrisTowles/towles-tool-rs.git",
      needs: 1,
      folders: [
        {
          name: "slot-0",
          dir: "/home/ctowles/code/p/towles-tool-rs-slot-0",
          branch: "feat/data-hub",
          isWorktree: false,
          filesChanged: 6,
          linesAdded: 88,
          linesRemoved: 20,
          commitsDelta: 2,
          needs: 0,
          sessions: [
            {
              id: "s0",
              name: "store-work",
              createdAt: 0,
              unseen: false,
              agentState: { agent: "claude", session: "slot-0", status: "busy", ts: 0 },
              agents: [],
            },
            { id: "s1", name: "shell 1", createdAt: 0, unseen: false, agentState: null, agents: [] },
          ],
        },
        {
          name: "slot-1",
          dir: "/home/ctowles/code/p/towles-tool-rs-slot-1",
          branch: "feat/agentboard-folder-rail",
          isWorktree: false,
          filesChanged: 12,
          linesAdded: 340,
          linesRemoved: 45,
          commitsDelta: 1,
          needs: 1,
          sessions: [
            {
              id: "s2",
              name: "webhook-fix",
              createdAt: 0,
              unseen: true,
              agentState: { agent: "claude", session: "slot-1", status: "waiting", ts: 0 },
              agents: [],
            },
            { id: "s3", name: "shell 1", createdAt: 0, unseen: false, agentState: null, agents: [] },
          ],
        },
      ],
    },
    {
      key: "https://github.com/ChrisTowles/toolbox.git",
      name: "toolbox",
      originUrl: "https://github.com/ChrisTowles/toolbox.git",
      needs: 0,
      folders: [
        {
          name: "toolbox",
          dir: "/home/ctowles/code/p/toolbox",
          branch: "main",
          isWorktree: false,
          filesChanged: 1,
          linesAdded: 3,
          linesRemoved: 2,
          commitsDelta: 0,
          needs: 0,
          sessions: [
            {
              id: "s4",
              name: "refactor",
              createdAt: 0,
              unseen: false,
              agentState: { agent: "claude", session: "toolbox", status: "busy", ts: 0 },
              agents: [],
            },
          ],
        },
      ],
    },
  ],
};

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
      // internals — show mock data instead of leaking unhandled rejections.
      if (!("__TAURI_INTERNALS__" in window)) {
        setState(MOCK_STATE);
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
