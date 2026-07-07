import { useEffect, useState } from "react";
import type { PrItem } from "./data";
import { invokeCmd, invokeOrThrow } from "./tauri";

/**
 * Client-side view of the agentboard bridge (`crates-tauri/tt-app/src/agentboard.rs`).
 * Mirrors the serialized `StatePayload` / `SessionData` (camelCase) that the
 * `ab_get_state` command returns and the `agentboard://state` event broadcasts.
 * Only the fields the screen renders are typed; the payload carries more.
 */

/** Invoke an `ab_*` Tauri command (thin alias over the shared invoker). */
export const abInvoke = invokeCmd;

/** Create a GitHub issue directly for the repo checked out at `dir` (`gh`
 * infers the repo from the folder's git remote). Returns the new issue's URL;
 * throws on failure so the caller can surface it (e.g. via toast). */
export const abCreateIssue = (dir: string, title: string) =>
  invokeOrThrow<string>("store_create_issue", { dir, title });

export type AgentStatus =
  "idle" | "busy" | "complete" | "error" | "waiting" | "interrupted";

/** Per-agent live details from the transcript tail (tokens, cache, model).
 * Mirrors the Rust `AgentEventDetails`; only the fields the UI renders. */
export type AgentEventDetails = {
  model?: string | null;
  contextUsed?: number | null;
  contextMax?: number | null;
  /** Epoch ms when the prompt cache expires; null/absent = no cache activity. */
  cacheExpiresAt?: number | null;
  /** 300_000 (5m) or 3_600_000 (1h). */
  cacheTtlMs?: number | null;
  lastActivityAt?: number | null;
};

export type AgentEvent = {
  agent: string;
  session: string;
  status: AgentStatus;
  ts: number;
  threadName?: string;
  unseen?: boolean;
  details?: AgentEventDetails | null;
};

/** One PTY shell inside a folder. "Agent" is a badge: `agentState` is set when
 * Claude (or another agent) is detected running in this PTY. */
export type SessionData = {
  id: string;
  name: string;
  createdAt: number;
  /** True when a PTY is currently running for this session (stamped by the
   * app from its terminal registry). False = the session record exists but
   * hasn't been started. */
  live: boolean;
  /** The shell's display name ("zsh", "bash", …), resolved once at PTY spawn
   * time (stamped by the app, same as `live`). Null until the session starts. */
  shellKind?: string | null;
  /** True when no PTY is attached but the shell still runs, detached, on the
   * app's session daemon (app was closed / pane replaced). Starting the
   * session reattaches it with its history. */
  detached?: boolean;
  unseen: boolean;
  agentState?: AgentEvent | null;
  agents: AgentEvent[];
  /** User-authored "what am I working toward here" — captured when starting
   * Claude, so the rail can explain why this session exists. */
  purpose?: string | null;
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
  /** User-authored "what am I working toward here" (persisted per folder). */
  purpose?: string | null;
  /** Agent-pushed progress/status (`ab_set_progress`/`ab_set_status`). Only
   * `progress.percent` is rendered today; the payload carries more. */
  metadata?: { progress?: { percent?: number | null } | null } | null;
};

/** A logical repo: the group of checkouts sharing a `git remote origin` URL. */
export type RepoData = {
  key: string;
  name: string;
  originUrl?: string | null;
  folders: FolderData[];
  needs: number;
};

/** One in-app window: a named tiling of pane session-ids. Scoped to a single
 * folder — a window may never hold panes from more than one checkout. */
export type AgWindow = { id: string; name: string; folderDir: string; panes: string[] };

/** The whole window layout. Frontend-owned: mutated locally, saved debounced
 * via `ab_save_windows`, hydrated once from `ab_get_state`. `activeWindows`
 * tracks the focused window per folder (keyed by `AgWindow.folderDir`). */
export type WindowsPayload = { windows: AgWindow[]; activeWindows: Record<string, string> };

export type StatePayload = {
  repos: RepoData[];
  theme?: string | null;
  preferredEditor: string;
  /** Context-% at/above which a cold session shows the compact nudge. */
  compactRecommendPercent: number;
  /** Persisted window layout (hydration source only — see WindowsPayload). */
  windows: WindowsPayload;
  /** Persisted folder-rail collapse/expand state, keyed by row key (hydration
   * source only — saved incrementally via `ab_save_collapsed`). Absent key ⇒
   * expanded. */
  collapsed: Record<string, boolean>;
  ts: number;
};

/** Window identity colors for the rail group tags + window-strip squares.
 * Deliberately distinct from the status hues (yellow/blue/red/green/orange)
 * and accents (violet/amber/sky) so a group tag never reads as a state. */
const WINDOW_COLORS = [
  "bg-teal-500",
  "bg-fuchsia-500",
  "bg-lime-500",
  "bg-rose-400",
  "bg-indigo-400",
];

export function windowColor(wins: AgWindow[], windowId: string): string {
  const i = wins.findIndex((w) => w.id === windowId);
  return i < 0 ? "bg-muted-foreground/40" : WINDOW_COLORS[i % WINDOW_COLORS.length];
}

/** The window containing a session's pane, if any. */
export function windowOf(wins: AgWindow[], sessionId: string): AgWindow | undefined {
  return wins.find((w) => w.panes.includes(sessionId));
}

/** A session is an "agent" session iff Claude is running in it right now. */
export function isAgent(s: SessionData): boolean {
  return s.agentState != null;
}

/** A session "needs you" when its agent is blocked on input or errored. */
export function sessionNeeds(s: SessionData): boolean {
  return s.agentState?.status === "waiting" || s.agentState?.status === "error";
}

/** A session should catch your eye when it needs you right now (`sessionNeeds`)
 * or when its agent reached a terminal state (done/errored/interrupted) you
 * haven't acknowledged yet (`unseen`, cleared by `ab_mark_seen` on select). A
 * plain `idle` agent — no news since you last looked — stays calm. */
export function sessionCatchesEye(s: SessionData): boolean {
  return sessionNeeds(s) || s.unseen;
}

/** A folder's currently-running (PTY-live) sessions. */
export function liveSessions(folder: FolderData): SessionData[] {
  return folder.sessions.filter((s) => s.live);
}

/** Display names of every live session across a repo's checkouts, for the
 * remove-repo confirmation copy. */
export function repoLiveSessionNames(repo: RepoData): string[] {
  return repo.folders.flatMap((f) => liveSessions(f).map((s) => sessionLabel(s)));
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
  if (!s.live) return s.detached ? "detached — still running" : "not started";
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

/** The `~/code/<scope>/` prefix of a checkout dir (`w/` work, `p/` personal,
 * `f/` fork), or null when the dir lives outside that layout. */
export function pathScope(dir: string): string | null {
  const m = dir.match(/\/code\/([a-z])\//);
  return m ? `${m[1]}/` : null;
}

/** The open PR for a folder's branch: exact branch match, scoped to the repo
 * via its origin URL (PR rows carry gh's `owner/name`, which both https and
 * ssh remote URLs contain). Origin-less repos match on branch alone. */
export function prForFolder(
  prs: PrItem[],
  originUrl: string | null | undefined,
  branch: string,
): PrItem | undefined {
  if (!branch) return undefined;
  const origin = originUrl?.toLowerCase();
  return prs.find(
    (p) => p.branch === branch && (!origin || origin.includes(p.repo.toLowerCase())),
  );
}

/** `0:04` / `3:20` / `1:02:30` — elapsed duration since a session started. */
export function fmtElapsed(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

// --- Cache & context health (Tier 3) ---

/** Percent of the context window used (0 when unknown). */
export function ctxPct(d: AgentEventDetails | null | undefined): number {
  if (!d?.contextUsed || !d.contextMax) return 0;
  return Math.round((d.contextUsed / d.contextMax) * 100);
}

/** A session is cache-cold when it never had cache activity or the TTL lapsed. */
export function isCold(d: AgentEventDetails | null | undefined, now: number): boolean {
  return !d?.cacheExpiresAt || now >= d.cacheExpiresAt;
}

/** The compact nudge: cold AND at/above the settings threshold. Warm-and-huge
 * is fine to keep going — the cost only bites on a cold resume. */
export function needsCompact(
  d: AgentEventDetails | null | undefined,
  now: number,
  thresholdPct: number,
): boolean {
  return d != null && ctxPct(d) >= thresholdPct && isCold(d, now);
}

/** Board-wide tally of running agents, for the nav badge and rail header:
 * "17 agents · 3 waiting · 1 busy · ❄2 to compact" at a glance. Counts only
 * sessions where an agent is detected running (`agentState` set). */
export type AgentRollup = {
  total: number;
  busy: number;
  waiting: number;
  error: number;
  /** Running agents that are cold + over the compact threshold. */
  compact: number;
};

export function agentRollup(
  repos: RepoData[],
  now: number,
  compactThresholdPct: number,
): AgentRollup {
  const r: AgentRollup = { total: 0, busy: 0, waiting: 0, error: 0, compact: 0 };
  for (const repo of repos)
    for (const f of repo.folders)
      for (const s of f.sessions) {
        const st = s.agentState?.status;
        if (!st) continue;
        r.total += 1;
        if (st === "busy") r.busy += 1;
        else if (st === "waiting") r.waiting += 1;
        else if (st === "error") r.error += 1;
        if (needsCompact(s.agentState?.details, now, compactThresholdPct)) r.compact += 1;
      }
  return r;
}

const EMPTY_WINDOWS: WindowsPayload = { windows: [], activeWindows: {} };

const EMPTY: StatePayload = {
  repos: [],
  preferredEditor: "",
  compactRecommendPercent: 30,
  windows: EMPTY_WINDOWS,
  collapsed: {},
  ts: 0,
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

// --- Session PTY writes ---

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Write raw bytes into a session's PTY. False when the PTY isn't running. */
export async function termWrite(termId: string, data: string): Promise<boolean> {
  if (!("__TAURI_INTERNALS__" in window)) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    await invoke("term_write", { termId, data });
    return true;
  } catch {
    return false;
  }
}

/** Write, retrying while the PTY spawns (a just-mounted terminal takes a beat
 * before `term_start` registers it). Gives up after ~3s. */
export async function termWriteRetry(termId: string, data: string): Promise<boolean> {
  for (let i = 0; i < 20; i++) {
    if (await termWrite(termId, data)) return true;
    await sleep(150);
  }
  return false;
}

/** Single-quote a string for safe injection into a shell command line typed
 * into a PTY (POSIX `'...'` escaping — embedded `'` becomes `'\''`). */
function shellQuote(text: string): string {
  return `'${text.replace(/'/g, `'\\''`)}'`;
}

/** The `claude` invocation for a session's PTY: bare, or with an initial
 * prompt passed as an argument so Claude starts working on it immediately
 * instead of waiting at an empty prompt. */
export function claudeCommand(prompt: string): string {
  const trimmed = prompt.trim();
  return trimmed ? `claude ${shellQuote(trimmed)}\r` : "claude\r";
}

// --- Session lifecycle + layout shared types ---

/** The lifecycle actions a session row can trigger. All are PTY writes — the
 * agent is whatever runs in the real shell, never a re-rendered proxy. */
export type SessionActions = {
  /** Mount + spawn the session's shell (no Claude). */
  start: (folderDir: string, s: SessionData) => void;
  /** Ensure the shell is live, then launch Claude in it. */
  startClaude: (folderDir: string, s: SessionData) => void;
  /** Interrupt Claude (Ctrl-C) then exit it (Ctrl-D). The shell survives. */
  stopClaude: (s: SessionData) => void;
  /** Send `/compact` to a Claude sitting at its prompt. */
  compactClaude: (s: SessionData) => void;
  /** Stop Claude, then launch a fresh session in the same shell. */
  restartClaude: (folderDir: string, s: SessionData) => void;
  close: (sessionId: string) => void;
  renameStart: (sessionId: string) => void;
  /** Remove the session's pane from its window (session stays in the rail). */
  ungroup: (sessionId: string) => void;
  /** Focus the window a session's group tag points at. */
  focusWindow: (windowId: string) => void;
};

/** Percent-rect for one pane in the active window's tiling: side-by-side up to
 * three across, a 2-column grid from four panes on. */
export type PaneRect = { left: number; top: number; width: number; height: number };

export function paneRects(n: number): PaneRect[] {
  if (n <= 0) return [];
  if (n <= 3) {
    const w = 100 / n;
    return Array.from({ length: n }, (_, i) => ({ left: i * w, top: 0, width: w, height: 100 }));
  }
  const rows = Math.ceil(n / 2);
  const h = 100 / rows;
  return Array.from({ length: n }, (_, i) => {
    const lastRowSolo = n % 2 === 1 && i === n - 1;
    return {
      left: lastRowSolo ? 0 : (i % 2) * 50,
      top: Math.floor(i / 2) * h,
      width: lastRowSolo ? 100 : 50,
      height: h,
    };
  });
}

/** Optimistic status shown for ~2.5s after a lifecycle action, until the
 * watcher's ground truth catches up on its next scan. */
export type Overlay = { status: AgentStatus; until: number };

export type Selected = { folderDir: string; sessionId: string } | null;

/** Drop any `activeWindows` entries pointing at a window that no longer
 * exists (or whose folder no longer matches) — windows are created lazily
 * per folder as sessions open, so there's no "at least one window" floor. */
export function normalizeWins(w: WindowsPayload): WindowsPayload {
  const activeWindows: Record<string, string> = {};
  for (const [folderDir, windowId] of Object.entries(w.activeWindows)) {
    if (w.windows.some((win) => win.id === windowId && win.folderDir === folderDir)) {
      activeWindows[folderDir] = windowId;
    }
  }
  return { windows: w.windows, activeWindows };
}

/** Wall clock ticking every `intervalMs` — drives cache-warmth countdowns.
 * 30s granularity keeps the rail calm (badges show minutes, not seconds). */
export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

/** A repo row in the manage-repos picker (from `ab_discover_repos`): every
 * repo under the scan roots, unioned with every repo already on the rail. */
export type RepoCandidate = { name: string; dir: string; active: boolean };

/** What a repo-remove confirmation (or immediate removal) needs to act on. */
export type RemoveTarget = { label: string; dirs: string[]; sessionIds: string[] };

/** A session about to get Claude launched in it, awaiting the "what are you
 * working toward?" prompt (see `commitStartClaude`). `restart` runs the
 * interrupt-then-relaunch dance first (a live Claude sits in the shell). */
export type StartClaudeTarget = {
  folderDir: string;
  sessionId: string;
  sessionName: string;
  restart: boolean;
};
