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
  unseen: boolean;
  agentState?: AgentEvent | null;
  agents: AgentEvent[];
  /** User-authored "what am I working toward here" — captured when starting
   * Claude, so the rail can explain why this session exists. */
  purpose?: string | null;
};

/** Tone hint on agent-pushed status/log lines (Rust `MetadataTone`). */
export type MetadataTone = "neutral" | "info" | "success" | "warn" | "error";

/** Agent-pushed metadata for a folder (`ab_set_status`/`ab_set_progress`/
 * `ab_log`, also reachable over MCP) — the agent's own words about what it's
 * doing, rendered read-only under the folder header. */
export type FolderMetadata = {
  status?: { text: string; tone?: MetadataTone | null; ts: number } | null;
  progress?: {
    percent?: number | null;
    label?: string | null;
  } | null;
  logs?: { message: string; tone?: MetadataTone | null; source?: string | null; ts: number }[];
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
  /** Commits on this branch that origin/main doesn't have. */
  commitsAhead: number;
  /** Commits on origin/main that this branch doesn't have. */
  commitsBehind: number;
  sessions: SessionData[];
  needs: number;
  /** User-authored "what am I working toward here" (persisted per folder). */
  purpose?: string | null;
  metadata?: FolderMetadata | null;
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

// --- Diff panes ---
// A window's `panes` normally hold session ids (`s<16 hex>` from the backend's
// `gen_id`). A folder's diff view rides the same tiling as a sentinel pane id
// (`~diff:<folderDir>` — `~` can never open a session id), so the diff renders
// *beside* the live terminals instead of covering them in a modal.

const DIFF_PANE_PREFIX = "~diff:";

/** The (per-folder) pane id of the folder's diff pane. */
export function diffPaneId(folderDir: string): string {
  return `${DIFF_PANE_PREFIX}${folderDir}`;
}

export function isDiffPane(paneId: string): boolean {
  return paneId.startsWith(DIFF_PANE_PREFIX);
}

/** The folder dir a diff pane id points at (null for session panes). */
export function diffPaneDir(paneId: string): string | null {
  return isDiffPane(paneId) ? paneId.slice(DIFF_PANE_PREFIX.length) : null;
}

// --- Pure window-layout reducers (unit-tested; the screen wraps them in
// `updateWins` for persistence) ---

/** Place a pane in its folder's focused window, creating a "primary" window if
 * the folder has none. A pane already hosted somewhere isn't moved — its
 * window becomes the folder's active one instead, so clicking a rail row
 * brings the existing pane into view rather than duplicating it. */
export function placePane(
  w: WindowsPayload,
  folderDir: string,
  paneId: string,
  newWindowId: () => string,
): WindowsPayload {
  const host = w.windows.find((win) => win.panes.includes(paneId));
  if (host) {
    return w.activeWindows[folderDir] === host.id
      ? w
      : { ...w, activeWindows: { ...w.activeWindows, [folderDir]: host.id } };
  }
  let windows = w.windows;
  let windowId = w.activeWindows[folderDir];
  if (!windows.some((win) => win.id === windowId && win.folderDir === folderDir)) {
    // Stale/missing active entry: reuse the folder's first existing window
    // before minting a new one — otherwise a dangling entry spawns a duplicate
    // "primary" beside the window the user already has.
    const existing = windows.find((win) => win.folderDir === folderDir);
    if (existing) {
      windowId = existing.id;
    } else {
      windowId = newWindowId();
      windows = [...windows, { id: windowId, name: "primary", folderDir, panes: [] }];
    }
  }
  return {
    windows: windows.map((win) =>
      win.id === windowId ? { ...win, panes: [...win.panes, paneId] } : win,
    ),
    activeWindows: { ...w.activeWindows, [folderDir]: windowId },
  };
}

/** Drop a pane from the window that holds it (pane ids are unique — session
 * ids globally, diff ids per folder — so at most one window matches). A window
 * is a tiling of panes, not a container worth keeping empty: when the pane was
 * the window's last, the window goes with it and the folder's active window
 * moves to a sibling — unless it's the folder's only window, which stays as
 * the landing surface for the next pane. */
export function dropPane(w: WindowsPayload, paneId: string): WindowsPayload {
  const host = w.windows.find((win) => win.panes.includes(paneId));
  if (!host) return w;
  const siblings = w.windows.filter(
    (win) => win.folderDir === host.folderDir && win.id !== host.id,
  );
  if (host.panes.length === 1 && siblings.length > 0) {
    const activeWindows = { ...w.activeWindows };
    if (activeWindows[host.folderDir] === host.id) {
      activeWindows[host.folderDir] = siblings[0].id;
    }
    return { windows: w.windows.filter((win) => win.id !== host.id), activeWindows };
  }
  return {
    ...w,
    windows: w.windows.map((win) =>
      win.id === host.id ? { ...win, panes: win.panes.filter((p) => p !== paneId) } : win,
    ),
  };
}

/** The folder dirs whose slice of the layout (their windows, in order, or
 * their active-window entry) differs between two payloads — exactly the
 * `touchedFolders` the backend's merge-by-folder save needs. */
export function changedFolderDirs(a: WindowsPayload, b: WindowsPayload): string[] {
  const dirs = new Set<string>([
    ...a.windows.map((win) => win.folderDir),
    ...b.windows.map((win) => win.folderDir),
    ...Object.keys(a.activeWindows),
    ...Object.keys(b.activeWindows),
  ]);
  const sig = (p: WindowsPayload, dir: string) =>
    JSON.stringify([
      p.windows.filter((win) => win.folderDir === dir),
      p.activeWindows[dir] ?? null,
    ]);
  return [...dirs].filter((d) => sig(a, d) !== sig(b, d));
}

/** Hydration-time sweep: drop every zero-pane window. Windows are created
 * lazily (`placePane` mints "primary" on demand), so an empty window restored
 * from disk is pure residue — it holds no panes, and its only state worth
 * missing is a name. Only safe at hydration: run mid-session it would eat a
 * window the user just created via "+ window" and hasn't filled yet. */
export function dropEmptyWindows(w: WindowsPayload): WindowsPayload {
  const windows = w.windows.filter((win) => win.panes.length > 0);
  return windows.length === w.windows.length
    ? w
    : normalizeWins({ windows, activeWindows: w.activeWindows });
}

/** Reconcile the persisted layout against what actually exists. The blob on
 * disk outlives its panes: sessions get removed by another app instance, a
 * repo comes off the rail with non-live session records, a crash beats the
 * debounced save — leaving ghost pane ids that hold a tile slot and render as
 * a dead dashed pane (so a fresh pane lands in spot two behind a corpse).
 *
 * Drops windows of folders not in `validFolderDirs`, then panes that are
 * neither a known session id nor a valid folder's diff pane. Windows *emptied
 * by this prune* vanish like a closed-out window (`dropPane`'s rule), keeping
 * one per folder when the prune emptied them all; a window that was already
 * empty going in is deliberate (freshly created via "+ window") and is never
 * touched. Returns `w` itself when nothing changed, so callers can cheaply
 * skip the save. */
export function pruneWins(
  w: WindowsPayload,
  validSessionIds: ReadonlySet<string>,
  validFolderDirs: ReadonlySet<string>,
): WindowsPayload {
  const pruned: AgWindow[] = [];
  const emptied = new Set<string>();
  for (const win of w.windows) {
    if (!validFolderDirs.has(win.folderDir)) continue;
    const panes = win.panes.filter((p) => {
      const dir = diffPaneDir(p);
      return dir !== null ? validFolderDirs.has(dir) : validSessionIds.has(p);
    });
    if (panes.length === 0 && win.panes.length > 0) emptied.add(win.id);
    pruned.push(panes.length === win.panes.length ? win : { ...win, panes });
  }
  const kept = pruned.filter((win) => {
    if (!emptied.has(win.id)) return true;
    const folderWins = pruned.filter((x) => x.folderDir === win.folderDir);
    if (folderWins.some((x) => !emptied.has(x.id))) return false;
    // The prune emptied every window of this folder — keep the active one.
    const keeper =
      folderWins.find((x) => x.id === w.activeWindows[win.folderDir]) ?? folderWins[0];
    return win.id === keeper.id;
  });
  const activeWindows: Record<string, string> = {};
  for (const win of kept) {
    if (win.folderDir in activeWindows) continue;
    const cur = w.activeWindows[win.folderDir];
    activeWindows[win.folderDir] =
      cur && kept.some((x) => x.folderDir === win.folderDir && x.id === cur) ? cur : win.id;
  }
  const next = { windows: kept, activeWindows };
  return changedFolderDirs(w, next).length === 0 ? w : next;
}

/** A session is an "agent" session iff Claude is running in it right now. */
export function isAgent(s: SessionData): boolean {
  return s.agentState != null;
}

/** A session "needs you" only when a shell actually exists for it (live PTY —
 * anything else is a stale record whose agent status can't be current) AND
 * its agent demands attention: blocked on input, errored, or its turn just
 * ended and you haven't looked yet (`unseen` terminal state, cleared by
 * `ab_mark_seen` on select). Mirrors `session_needs` in
 * `crates/tt-agentboard/src/bridge.rs` — keep the two in lockstep. */
export function sessionNeeds(s: SessionData): boolean {
  if (!s.live) return false;
  const st = s.agentState?.status;
  if (st === "waiting" || st === "error") return true;
  return s.unseen && (st === "complete" || st === "interrupted");
}

/** A session should catch your eye when it needs you right now (`sessionNeeds`)
 * or when its agent reached a terminal state (done/errored/interrupted) you
 * haven't acknowledged yet (`unseen`, cleared by `ab_mark_seen` on select). A
 * plain `idle` agent — no news since you last looked — stays calm. */
export function sessionCatchesEye(s: SessionData): boolean {
  return sessionNeeds(s) || s.unseen;
}

/** The next (or previous) session that catches the eye (`sessionCatchesEye`),
 * board-wide, in the same repo → folder → session order the rail renders.
 * `fromSessionId` anchors the cycle — the result is the nearest match after
 * (or before) it in that order, wrapping around; `null` (nothing selected, or
 * the id isn't found) starts from the very beginning/end. Returns `null` when
 * nothing currently catches the eye. */
export function cycleNeedsYou(
  repos: RepoData[],
  fromSessionId: string | null,
  direction: "next" | "prev",
): SessionData | null {
  const all: SessionData[] = [];
  for (const r of repos) for (const f of r.folders) for (const s of f.sessions) all.push(s);

  const targetIndexes = all
    .map((s, i) => (sessionCatchesEye(s) ? i : -1))
    .filter((i) => i !== -1);
  if (targetIndexes.length === 0) return null;

  const fromIndex = fromSessionId ? all.findIndex((s) => s.id === fromSessionId) : -1;

  const chosen =
    direction === "next"
      ? (targetIndexes.find((i) => i > fromIndex) ?? targetIndexes[0])
      : ([...targetIndexes].reverse().find((i) => i < fromIndex) ??
        targetIndexes[targetIndexes.length - 1]);

  return all[chosen];
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
  if (!s.live) return "not started";
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

/** How long after its last sign of agent life a folder still counts as
 * active for the hide-inactive filter, so stopping a session doesn't make
 * its folder vanish from the rail the same instant. */
export const QUIET_GRACE_MS = 45 * 60_000;

/** The newest agent-activity timestamp a folder carries: agent events on its
 * sessions (current state + history) and agent-pushed folder metadata
 * (status/logs). 0 when the folder has never seen agent activity — a
 * never-started session record carries no timestamps, so a stale worktree
 * can't pin itself visible forever. */
export function folderLastActivityAt(f: FolderData): number {
  let last = 0;
  for (const s of f.sessions) {
    for (const ev of [s.agentState, ...s.agents]) {
      if (!ev) continue;
      last = Math.max(last, ev.ts, ev.details?.lastActivityAt ?? 0);
    }
  }
  if (f.metadata?.status) last = Math.max(last, f.metadata.status.ts);
  for (const l of f.metadata?.logs ?? []) last = Math.max(last, l.ts);
  return last;
}

/** A folder (checkout) is "quiet" when nothing about it needs attention right
 * now: no live session, no session that catches the eye (waiting/errored/
 * unseen), no unpushed local commits or dirty working tree, and no agent
 * activity within the `QUIET_GRACE_MS` grace window (so a folder eases off
 * the rail a while after work stops, rather than the moment it does). Being
 * *behind* origin (`commitsBehind > 0`) doesn't count — that's just staleness,
 * not work in progress. A worktree that was created but never had a session
 * opened in it falls out of this naturally (empty `sessions`, clean tree, no
 * activity timestamps) — no special case needed. Richer than "no live
 * session": a folder can be mid-work (dirty tree, unpushed commits, a
 * finished-but-unseen turn) with nothing currently *running*. */
export function isFolderQuiet(f: FolderData, now: number): boolean {
  return (
    liveSessions(f).length === 0 &&
    f.filesChanged === 0 &&
    f.commitsAhead === 0 &&
    f.sessions.every((s) => !sessionCatchesEye(s)) &&
    now - folderLastActivityAt(f) >= QUIET_GRACE_MS
  );
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

/** How far before expiry the cache counts as "expiring": 2m on the 5-minute
 * cache, 10m on the 1-hour cache — enough headroom to nudge the session (any
 * request re-warms the cache) before a resume goes full-price. */
export function cacheWarnMs(ttlMs: number | null | undefined): number {
  return ttlMs === 3_600_000 ? 600_000 : 120_000;
}

/** Still warm, but inside the warn window — one nudge away from going cold. */
export function isCacheExpiring(d: AgentEventDetails | null | undefined, now: number): boolean {
  if (!d?.cacheExpiresAt || isCold(d, now)) return false;
  return d.cacheExpiresAt - now <= cacheWarnMs(d.cacheTtlMs);
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
  /** Running agents whose warm cache is inside the warn window. */
  expiring: number;
};

export function agentRollup(
  repos: RepoData[],
  now: number,
  compactThresholdPct: number,
): AgentRollup {
  const r: AgentRollup = { total: 0, busy: 0, waiting: 0, error: 0, compact: 0, expiring: 0 };
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
        if (isCacheExpiring(s.agentState?.details, now)) r.expiring += 1;
      }
  return r;
}

/** The one dot color for a whole rollup — same busy/waiting/error precedence
 * as a collapsed rail row, so a collapsed nav icon's badge never disagrees
 * with the rail's own collapsed dots. Null when nothing is running. */
export function rollupAlertColor(r: AgentRollup): string | null {
  if (r.error > 0) return "bg-red-500";
  if (r.waiting > 0) return "bg-blue-500";
  if (r.busy > 0) return "bg-yellow-500";
  if (r.total > 0) return "bg-emerald-500";
  return null;
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
export function shellQuote(text: string): string {
  return `'${text.replace(/'/g, `'\\''`)}'`;
}

/** The `claude` invocation for a session's PTY: bare, or with an initial
 * prompt passed as an argument so Claude starts working on it immediately
 * instead of waiting at an empty prompt. */
export function claudeCommand(prompt: string): string {
  const trimmed = prompt.trim();
  return trimmed ? `claude ${shellQuote(trimmed)}\r` : "claude\r";
}

/** The `claude` invocation to resume a past session (from the Claude Sessions
 * history) in place, typed into the folder's PTY. */
export function claudeResumeCommand(sessionId: string): string {
  return `claude --resume ${shellQuote(sessionId)}\r`;
}

/** Result of [`abOpenSessionForCwd`]: the resolved repo dir + the new
 * Agentboard session id, so the caller can select it immediately. */
export type OpenedSession = { folderDir: string; sessionId: string };

/** Resolve a Claude Code session's real `cwd` to an Agentboard repo (adding
 * it to the rail first if it isn't already registered) and open a new
 * session there. Throws on failure so the caller can surface it. */
export const abOpenSessionForCwd = (cwd: string) =>
  invokeOrThrow<OpenedSession>("ab_open_session_for_cwd", { cwd });

/** A cross-screen handoff: "select this folder/session in Agentboard, then
 * type a resume command into it." Agentboard may not be mounted yet when the
 * request is made (e.g. the Claude Sessions screen is the active tab), so
 * this can't be a plain function call — it's a one-shot mailbox: `requestOpenSession`
 * either delivers immediately (a listener is already mounted) or stashes the
 * request for Agentboard's mount effect to consume via `consumePendingOpenSession`. */
export type PendingOpenSession = {
  folderDir: string;
  sessionId: string;
  resumeId: string;
  label: string;
};

let pendingOpenSession: PendingOpenSession | null = null;
const openSessionListeners = new Set<(req: PendingOpenSession) => void>();

export function requestOpenSession(req: PendingOpenSession) {
  if (openSessionListeners.size > 0) {
    for (const l of openSessionListeners) l(req);
    return;
  }
  pendingOpenSession = req;
}

export function consumePendingOpenSession(): PendingOpenSession | null {
  const req = pendingOpenSession;
  pendingOpenSession = null;
  return req;
}

export function onOpenSessionRequest(cb: (req: PendingOpenSession) => void): () => void {
  openSessionListeners.add(cb);
  return () => openSessionListeners.delete(cb);
}

/**
 * A read-only "reveal this folder/session in Agentboard" handoff — the command
 * palette's jump-to-repo/session entries. Unlike {@link requestOpenSession},
 * this only focuses/selects (no PTY writes, no `claude --resume`): a `folder`
 * request focuses the checkout, a `session` request also selects that session's
 * pane. Same one-shot-mailbox shape as the open-session bridge, because
 * Agentboard may not be mounted yet when the palette fires (its screen mounts
 * on first visit): deliver now if a listener is mounted, else stash for the
 * screen's mount effect to consume via {@link consumePendingAgentboardNav}.
 */
export type AgentboardNav =
  | { kind: "folder"; folderDir: string }
  | { kind: "session"; folderDir: string; sessionId: string };

let pendingNav: AgentboardNav | null = null;
const navListeners = new Set<(req: AgentboardNav) => void>();

export function requestAgentboardNav(req: AgentboardNav) {
  if (navListeners.size > 0) {
    for (const l of navListeners) l(req);
    return;
  }
  pendingNav = req;
}

export function consumePendingAgentboardNav(): AgentboardNav | null {
  const req = pendingNav;
  pendingNav = null;
  return req;
}

export function onAgentboardNavRequest(cb: (req: AgentboardNav) => void): () => void {
  navListeners.add(cb);
  return () => navListeners.delete(cb);
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
