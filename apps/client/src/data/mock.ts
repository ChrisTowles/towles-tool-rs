import type {
  AgentBoardState,
  AgentDisplay,
  ReorderDelta,
  SessionData,
} from "../types";
import { DEFAULT_THEME } from "../lib/themes";
import type { Commands, StateSource } from "./StateSource";

const TICK_MS = 1500;
const MIN = 60_000;

/** Key identifying a dismissible agent instance. */
function agentKey(session: string, agent: string, threadId?: string): string {
  return `${session}::${agent}::${threadId ?? ""}`;
}

/** Last path segment of an absolute dir (the repo's display name). */
function basename(path: string): string {
  const parts = path.replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || path;
}

/**
 * An in-memory AgentBoard backend that both emits an evolving demo snapshot on
 * a timer AND accepts client commands (mutating the same store). It implements
 * both `StateSource` and `Commands` so `App` can wire the whole UI to a single
 * mock. A real `TauriStateSource`/`TauriCommands` pair drops in later with the
 * same interfaces (see TauriStateSource.ts).
 */
export class MockBackend implements StateSource, Commands {
  private listeners = new Set<(s: AgentBoardState) => void>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private tick = 0;
  private theme = DEFAULT_THEME;

  // Command overlays applied on top of the scripted scenario.
  private removedRepos = new Set<string>();
  private seenSessions = new Set<string>();
  private dismissedAgents = new Set<string>();
  private addedRepos: string[] = [];
  private order: string[] = [];
  private errorListeners = new Set<(m: string) => void>();

  private latest: AgentBoardState | null = null;

  subscribe(listener: (s: AgentBoardState) => void): () => void {
    this.listeners.add(listener);
    if (this.latest) listener(this.latest);
    return () => this.listeners.delete(listener);
  }

  start(): void {
    if (this.timer) return;
    this.emit();
    this.timer = setInterval(() => {
      this.tick++;
      this.emit();
    }, TICK_MS);
  }

  stop(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  // --- Commands ---

  markSeen(name: string): void {
    this.seenSessions.add(name);
    this.emit();
  }

  dismissAgent(session: string, agent: string, threadId?: string): void {
    this.dismissedAgents.add(agentKey(session, agent, threadId));
    this.emit();
  }

  reorderSession(name: string, delta: ReorderDelta): void {
    const names = this.currentOrder();
    const from = names.indexOf(name);
    if (from < 0) return;
    names.splice(from, 1);
    let to = from;
    if (delta === "up") to = Math.max(0, from - 1);
    else if (delta === "down") to = Math.min(names.length, from + 1);
    else if (delta === "top") to = 0;
    else if (delta === "bottom") to = names.length;
    names.splice(to, 0, name);
    this.order = names;
    this.emit();
  }

  setTheme(theme: string): void {
    this.theme = theme;
    this.emit();
  }

  addRepo(path: string): void {
    const name = basename(path);
    // Un-remove if it was previously removed; otherwise append.
    this.removedRepos.delete(name);
    if (!this.addedRepos.includes(path) && !this.scenario().some((s) => s.name === name)) {
      this.addedRepos.push(path);
    }
    this.emit();
  }

  removeRepo(name: string): void {
    this.removedRepos.add(name);
    this.addedRepos = this.addedRepos.filter((p) => basename(p) !== name);
    this.emit();
  }

  refresh(): void {
    this.emit();
  }

  onError(listener: (m: string) => void): () => void {
    this.errorListeners.add(listener);
    return () => this.errorListeners.delete(listener);
  }

  // --- Snapshot assembly ---

  /** Minimal cards for repos added at runtime via `addRepo`. */
  private addedSessions(): SessionData[] {
    return this.addedRepos.map((path) => ({
      name: basename(path),
      dir: path,
      branch: "main",
      filesChanged: 0,
      linesAdded: 0,
      linesRemoved: 0,
      commitsDelta: 0,
      unseen: false,
      agentState: null,
      agents: [],
      metadata: null,
    }));
  }

  private currentOrder(): string[] {
    const base = [...this.scenario(), ...this.addedSessions()].map((s) => s.name);
    if (this.order.length === 0) return base;
    const known = this.order.filter((n) => base.includes(n));
    const rest = base.filter((n) => !this.order.includes(n));
    return [...known, ...rest];
  }

  private emit(): void {
    const now = Date.now();
    let sessions = [...this.scenario(), ...this.addedSessions()];

    // Overlays: removed repos, dismissed agents, mark-seen.
    sessions = sessions
      .filter((s) => !this.removedRepos.has(s.name))
      .map((s) => {
        const agents = s.agents.filter(
          (a) => !this.dismissedAgents.has(agentKey(a.session, a.agent, a.threadId)),
        );
        const seen = this.seenSessions.has(s.name);
        const agentState = agents.some((a) => a === s.agentState) ? s.agentState : agents[0] ?? null;
        return { ...s, agents, unseen: seen ? false : s.unseen, agentState };
      });

    // Apply custom order.
    if (this.order.length > 0) {
      const idx = new Map(this.currentOrder().map((n, i) => [n, i]));
      sessions = [...sessions].sort(
        (a, b) => (idx.get(a.name) ?? 0) - (idx.get(b.name) ?? 0),
      );
    }

    this.latest = { sessions, theme: this.theme, ts: now };
    for (const l of this.listeners) l(this.latest);
  }

  /**
   * The scripted scenario, recomputed each tick against the live clock so
   * elapsed timers, loop countdowns, and cache windows all move. Command
   * overlays are applied on top in `emit()`.
   */
  private scenario(): SessionData[] {
    const now = Date.now();
    const t = this.tick;

    // Repo 1: an active Claude session that accrues subagents then finishes.
    const r1Done = t % 12 >= 7; // running for 7 ticks, done for 5, then loops
    const r1Subs = Math.min(2, Math.max(0, (t % 12) - 2));
    const r1Agent: AgentDisplay = r1Done
      ? {
          agent: "claude",
          session: "towles-tool-primary",
          status: "complete",
          ts: now,
          threadId: "t-main",
          threadName: "port the agentboard TUI to React",
          unseen: true,
        }
      : {
          agent: "claude",
          session: "towles-tool-primary",
          status: "busy",
          ts: now,
          threadId: "t-main",
          threadName: "port the agentboard TUI to React",
          details: {
            model: "claude-opus-4-8",
            lastTool: ["Read", "Edit", "Bash", "Grep"][t % 4],
            lastActivityAt: now - ((t % 7) + 1) * 1000,
            cacheExpiresAt: now + 45 * MIN,
            subagents: Array.from({ length: r1Subs }, (_, i) => ({
              agentType: ["Explore", "general-purpose"][i % 2],
              description: [
                "map the theme palette usage across components",
                "sweep for inline color literals to replace",
              ][i % 2],
            })),
          },
        };

    // Repo 2: a waiting agent + programmatic metadata whose progress advances.
    const cur = (t % 5) + 1;
    const r2Agent: AgentDisplay = {
      agent: "claude",
      session: "towles-tool-slot-1",
      status: "idle",
      ts: now,
      threadId: "t-slot1",
      threadName: "run the vitest suite and report failures",
    };

    // Repo 3: a self-paced /loop counting down to its next wake.
    const wakeIn = 45_000 - (t % 30) * 1500;
    const r3Agent: AgentDisplay = {
      agent: "claude",
      session: "dotfiles",
      status: "busy",
      ts: now,
      threadId: "t-loop",
      threadName: "babysit the CI run until green",
      details: {
        model: "claude-sonnet-5",
        lastActivityAt: now - 4000,
        cacheExpiresAt: now + 12 * MIN,
        loop: wakeIn > 0 ? { nextWakeAt: now + wakeIn, reason: "poll gh checks" } : undefined,
      },
    };

    // Repo 4: an error the user hasn't acknowledged (unseen ● in red).
    const r4Agent: AgentDisplay = {
      agent: "claude",
      session: "toolbox",
      status: "error",
      ts: now,
      threadId: "t-err",
      threadName: "regenerate the release changelog",
      unseen: true,
    };

    return [
      {
        name: "towles-tool-primary",
        dir: "/home/ctowles/code/p/towles-tool",
        branch: "feat/agentboard-ui",
        filesChanged: 14,
        linesAdded: 862,
        linesRemoved: 37,
        commitsDelta: 2,
        unseen: r1Done,
        agentState: r1Agent,
        agents: [r1Agent],
        metadata: null,
      },
      {
        name: "towles-tool-slot-1",
        dir: "/home/ctowles/code/p/towles-tool-repos/towles-tool-slot-1",
        branch: "main",
        filesChanged: 0,
        linesAdded: 0,
        linesRemoved: 0,
        commitsDelta: 0,
        unseen: false,
        agentState: r2Agent,
        agents: [r2Agent],
        metadata: {
          status: { text: "running tests", tone: "info", ts: now },
          progress: { current: cur, total: 5, label: "vitest", ts: now },
          logs: [
            { message: "lib/derived.test.ts passed", tone: "success", source: "vitest", ts: now },
          ],
        },
      },
      {
        name: "dotfiles",
        dir: "/home/ctowles/code/p/dotfiles",
        branch: "main",
        filesChanged: 1,
        linesAdded: 6,
        linesRemoved: 1,
        commitsDelta: -1,
        unseen: false,
        agentState: r3Agent,
        agents: [r3Agent],
        metadata: null,
      },
      {
        name: "toolbox",
        dir: "/home/ctowles/code/p/toolbox",
        branch: "fix/changelog",
        filesChanged: 3,
        linesAdded: 12,
        linesRemoved: 40,
        commitsDelta: 0,
        unseen: true,
        agentState: r4Agent,
        agents: [r4Agent],
        metadata: null,
      },
      {
        // A minimal card: a repo with no agents and no diff (edge state).
        name: "blog",
        dir: "/home/ctowles/code/p/blog",
        branch: "draft/2026-agentboard",
        filesChanged: 0,
        linesAdded: 0,
        linesRemoved: 0,
        commitsDelta: 0,
        unseen: false,
        agentState: null,
        agents: [],
        metadata: null,
      },
    ];
  }
}
