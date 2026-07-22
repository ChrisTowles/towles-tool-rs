import { useEffect, useState } from "react";
import type { Result } from "better-result";
import type { z } from "zod";
import type { PrItem } from "./data";
import type { IpcError } from "./errors";
import type { LaunchConfigStatus } from "./launch";
import type { RepoMeta } from "./repo-identity";
import { OpenedSessionSchema } from "./schemas/agentboard";
import { TaskBlockerSchema, TaskRemoveOutcomeSchema } from "./schemas/task";
import { invoke } from "./tauri";

/**
 * Client-side view of the agentboard bridge (`crates-tauri/tt-app/src/agentboard.rs`).
 * Mirrors the serialized `StatePayload` / `SessionData` (camelCase) that the
 * `ab_get_state` command returns and the `agentboard://state` event broadcasts.
 * Only the fields the screen renders are typed; the payload carries more.
 */

/** Create a GitHub issue directly for the repo checked out at `dir` (`gh`
 * infers the repo from the folder's git remote). Resolves the new issue's URL,
 * or the failure for the caller to surface (e.g. via toast). */
export const abCreateIssue = (dir: string, title: string) =>
  invoke<string>("store_create_issue", { dir, title });

/** Outcome of `abSyncRepo` (mirrors the Rust `RepoSyncResult`). `started` is
 * `false` only when a sync for this dir was already in flight — a deduped
 * no-op the caller should ignore quietly. Otherwise `ok`/`count`/`message`
 * summarize the combined issues+PRs collector run. */
export type RepoSyncResult = {
  started: boolean;
  ok: boolean;
  count: number;
  message?: string | null;
};

/** Force the repo checked out at `dir` to sync its issues + PRs from GitHub
 * right now, bypassing the collector poll cadence — the rail's "Sync now"
 * action, for pulling in updates the poll hasn't picked up yet. Scoped to
 * this one repo; never touches other tracked repos' cached rows. A Tauri-level
 * failure is the `Err` side, for the caller to toast; a collector-level failure
 * (e.g. `gh` auth expired) comes back as `ok: false` with `message` instead. */
export const abSyncRepo = (dir: string) => invoke<RepoSyncResult>("store_sync_repo", { dir });

/** Label a session in the rail with what it exists for — the task's goal, the
 * prompt Claude was started on, or the dev-server command. A session's purpose
 * is a property of the session, not of whatever started it: every caller that
 * knows why a session was opened sets it, including the ones that never launch
 * anything into the PTY. */
export const abSetSessionPurpose = (id: string, text: string | null) =>
  invoke("ab_set_session_purpose", { id, text });

export type AgentStatus = "idle" | "busy" | "complete" | "error" | "waiting" | "interrupted";

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

/** One port a session's shell saw in its folder's `.env` at spawn time that
 * the file now claims differently — e.g. a sibling task's re-render rotated
 * it out from under an already-running pane. Mirrors the Rust `PortDrift`
 * (`crates/tt-agentboard/src/env_drift.rs`). */
export type PortDrift = { key: string; spawnedPort: number; currentPort: number };

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
  /** Epoch ms when this session first entered "needs you" (`sessionNeeds`),
   * held across snapshots by the backend so its waiting-age is stable; null
   * when it doesn't need you right now. Orders the attention feed oldest-first
   * and drives the "waiting 12m" row label. Mirrors `needs_since_ms` in
   * `crates/tt-agentboard/src/types.rs`. */
  needsSinceMs?: number | null;
  agentState?: AgentEvent | null;
  agents: AgentEvent[];
  /** Echo of the prompt Claude was launched with, auto-captured at launch so
   * the rail's hover tooltip can explain why this session exists. Read-only —
   * nothing in the UI edits it. */
  purpose?: string | null;
  /** Ports this session's shell saw at spawn time that its folder's `.env`
   * now claims differently (stamped by the app). Omitted from the wire
   * payload — and so absent here — when nothing has drifted or the session
   * isn't live. */
  portDrift?: PortDrift[];
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

/** How git decided a branch's work reached its base. The wire mirror of
 * `LandedVia::label()` (`crates/tt-tasks/src/landed.rs`) — every value the
 * backend can put in {@link FolderData.landed}. */
export type LandedVia = "merged" | "rebase-merged" | "squash-merged" | "upstream gone";

/** One checkout of a repo on disk (a clone, worktree, or task). */
export type FolderData = {
  name: string;
  dir: string;
  /** True when `dir` no longer exists on disk — a tracked repo whose checkout
   * was moved or deleted. Rendered as a dimmed "ghost" with an Untrack action. */
  dirMissing: boolean;
  branch: string;
  isWorktree: boolean;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  /** Commits on this branch that `comparedBase` doesn't have. */
  commitsAhead: number;
  /** Commits on `comparedBase` that this branch doesn't have. */
  commitsBehind: number;
  /** True when the working tree has uncommitted changes (staged, unstaged,
   * or untracked). Unlike `filesChanged` — the branch's whole *committed*
   * diff vs `comparedBase`, which stays nonzero for any real feature branch
   * even once it's merged — this is the actual "no uncommitted changes"
   * fact a safe-to-delete check needs. */
  dirty: boolean;
  /** Of `commitsAhead`, how many haven't landed on `comparedBase` yet
   * (patch-equivalence via `git cherry`, not SHA reachability). 0 once every
   * commit on this branch has landed there — even after a rebase/squash
   * merge gave the landed commits new SHAs, which `commitsAhead` can never
   * see past (it stays nonzero forever in that case). See
   * `folderHoldsNoWork`. */
  commitsUnlanded: number;
  /** How this branch's work reached `comparedBase` — `"merged"` (a merge
   * commit), `"rebase-merged"` or `"squash-merged"` — and `null` while it
   * hasn't fully landed. Mirrors `LandedVia::label()`
   * (`crates/tt-tasks/src/landed.rs`), which explains why no single git signal
   * answers this.
   *
   * {@link LandedVia}'s fourth label, `"upstream gone"`, is a real value in
   * Rust but never arrives here: it only says the remote branch vanished, which
   * looks identical whether the branch merged or was deleted unmerged, so
   * `compute_git_info` suppresses it rather than let a badge claim a merge git
   * can't prove.
   *
   * This is git's own proof that a branch is finished, independent of GitHub:
   * a locally merged branch that never had a PR still reports here, and a
   * merged PR says nothing about commits made since. `commitsUnlanded` is the
   * *whether*; this is the *how*. */
  landed: LandedVia | null;
  sessions: SessionData[];
  needs: number;
  /** Branch the diff pane's "vs main" mode compares against, overriding the
   * origin/main-or-master auto-detect (persisted per folder). */
  baseBranch?: string | null;
  /** For a worktree only: the ref it was actually created from (its
   * `.tt-task` marker). What the diff pane auto-compares against when
   * `baseBranch` has no manual override — `null` for a non-task checkout. */
  taskBaseBranch?: string | null;
  /** The ref `filesChanged`/`linesAdded`/`linesRemoved`/`commitsAhead`/
   * `commitsBehind` were actually measured against, e.g. `"origin/main"` or
   * `"origin/docs/readme-task-clean"` — always matches what the diff pane's
   * "vs main" mode shows. Empty until the folder's git stats are computed
   * at least once. */
  comparedBase?: string;
  metadata?: FolderMetadata | null;
  /** True when a live session in this folder has drifted ports — bubbles
   * `SessionData.portDrift` up for the rail badge. */
  hasPortDrift: boolean;
  /** True when this checkout has a Claude Desktop `.claude/launch.json` —
   * gates the rail's dev-servers button and picks the pane-header button's
   * dimmed/how-to state (`components/dev-servers.tsx`); the configs
   * themselves are fetched on demand via `launch_configs`. */
  hasLaunchConfig: boolean;
  /** Forced-quiet override (persisted per folder, `ab_set_folder_quiet`) —
   * `isFolderQuiet` treats this folder as quiet under the "hide inactive"
   * rail filter regardless of its actual activity signals above. One flag
   * whether it got set by hand or some other way; nothing here distinguishes
   * "manual" from any other source. */
  quiet: boolean;
};

/** The one definition of "this folder's working tree measurably changed" —
 * the diff pane refetches on it. Extend it here, not inline in a component,
 * so every consumer moves together. */
export function folderStatsKey(folder: FolderData): string {
  return `${folder.filesChanged}:${folder.linesAdded}:${folder.linesRemoved}:${folder.commitsAhead}`;
}

/** One commit ahead of `comparedBase`, with its own line-count diff — not the
 * folder's cumulative `linesAdded`/`linesRemoved`. Mirrors the Rust
 * `CommitStat` (`crates/tt-agentboard/src/git_info.rs`), returned oldest
 * first by `ab_get_commit_stats`. */
export type CommitStat = {
  sha: string;
  subject: string;
  linesAdded: number;
  linesRemoved: number;
};

/** `comparedBase` with its `origin/` prefix stripped for display, e.g.
 * `"origin/main"` → `"main"`. Falls back to `"main"` before the backend has
 * computed anything yet. Deliberately the opposite of the new-task form's
 * base label (`BaseBranchesSchema` in lib/schemas/task.ts), which *adds*
 * `origin/`: here the compared ref is always the freshest one the backend
 * found and the local/origin distinction is noise, while the form's whole
 * point is that you'll branch from origin's tip, not stale local history.
 * Don't "unify" them. */
export function comparedBaseLabel(folder: Pick<FolderData, "comparedBase">): string {
  const base = folder.comparedBase?.trim();
  if (!base) return "main";
  return base.startsWith("origin/") ? base.slice("origin/".length) : base;
}

/** A logical repo: a checkout plus every other rail folder that's a `git
 * worktree` sibling of it (same git common dir), whether that sibling is
 * explicitly tracked or only discovered via `git worktree list` (see
 * `RepoData` in `crates/tt-agentboard/src/types.rs`). Folders never merge
 * into one row merely for sharing an origin remote — only an actual
 * worktree relationship does. */
export type RepoData = {
  key: string;
  /** Absolute dir of the checkout this row is anchored to — the dir also
   * embedded in `key`, as a field so readers never parse it back out. The
   * key repo identity (`meta`) is stored under. */
  dir: string;
  name: string;
  originUrl?: string | null;
  folders: FolderData[];
  needs: number;
  /** User-chosen icon/color for this repo (Settings → Agentboard). Absent —
   * or absent fields — means "render exactly as an unthemed repo": never
   * synthesize a color from the name. Values are untrusted; resolve them
   * through `lib/repo-identity.ts`. */
  meta?: RepoMeta;
};

/** A window's tiled pane ids: always at least one — the empty-pane state is
 * unrepresentable. Windows are minted lazily around their first pane
 * (`placePane`, the "+ window" flow) and die with their last (`dropPane`,
 * `pruneWins`); blobs persisted before this rule may still hold paneless
 * windows, which `hydrateWins` sweeps at the parse boundary. */
export type Panes = [string, ...string[]];

/** Parse a plain id list into `Panes` (`null` when empty) — the one blessed
 * spot where `string[]` narrows to the non-empty pane type. */
export function toPanes(ids: string[]): Panes | null {
  return ids.length > 0 ? (ids as Panes) : null;
}

/** One in-app window: a named tiling of pane session-ids. Scoped to a single
 * folder — a window may never hold panes from more than one checkout. `cols`
 * holds user-dragged column widths in per-mille of the tiling width (summing
 * to `COL_TOTAL`, one entry per column of the current layout — see
 * `colCount`); absent or mismatched (the pane count changed since the drag)
 * means equal columns. */
export type AgWindow = {
  id: string;
  name: string;
  folderDir: string;
  panes: Panes;
  cols?: number[];
};

/** The whole window layout. Frontend-owned: mutated locally, saved debounced
 * via `ab_save_windows`, hydrated once from `ab_get_state`. `activeWindows`
 * tracks the focused window per folder (keyed by `AgWindow.folderDir`). */
export type WindowsPayload = { windows: AgWindow[]; activeWindows: Record<string, string> };

/** `AgWindow` as persisted / sent over the wire, before parsing: `panes` may
 * be empty in blobs written before empty windows became unrepresentable. */
export type WireWindow = Omit<AgWindow, "panes"> & { panes: string[] };
export type WireWindowsPayload = { windows: WireWindow[]; activeWindows: Record<string, string> };

export type StatePayload = {
  repos: RepoData[];
  theme?: string | null;
  preferredEditor: string;
  /** Context-% at/above which a cold session shows the compact nudge. */
  compactRecommendPercent: number;
  /** Persisted window layout (hydration source only — parse with
   * `hydrateWins`; see WindowsPayload). */
  windows: WireWindowsPayload;
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

// --- Folder panes (diff, files) ---
// A window's `panes` normally hold session ids (`s<16 hex>` from the backend's
// `gen_id`). A folder's diff and files views ride the same tiling as sentinel
// pane ids (`~diff:<folderDir>` / `~files:<folderDir>` — `~` can never open a
// session id), so they render *beside* the live terminals instead of covering
// them in a modal.

const DIFF_PANE_PREFIX = "~diff:";
const FILES_PANE_PREFIX = "~files:";
const PREVIEW_PANE_PREFIX = "~preview:";
const EXIT_PANE_PREFIX = "~exit:";

/** The (per-folder) pane id of the folder's diff pane. */
export function diffPaneId(folderDir: string): string {
  return `${DIFF_PANE_PREFIX}${folderDir}`;
}

export function isDiffPane(paneId: string): boolean {
  return paneId.startsWith(DIFF_PANE_PREFIX);
}

/** The folder dir a diff pane id points at (null otherwise). */
export function diffPaneDir(paneId: string): string | null {
  return isDiffPane(paneId) ? paneId.slice(DIFF_PANE_PREFIX.length) : null;
}

/** The (per-folder) pane id of the folder's files pane. */
export function filesPaneId(folderDir: string): string {
  return `${FILES_PANE_PREFIX}${folderDir}`;
}

export function isFilesPane(paneId: string): boolean {
  return paneId.startsWith(FILES_PANE_PREFIX);
}

/** The folder dir a files pane id points at (null otherwise). */
export function filesPaneDir(paneId: string): string | null {
  return isFilesPane(paneId) ? paneId.slice(FILES_PANE_PREFIX.length) : null;
}

/**
 * Resolve a terminal file-link path to a files-pane path relative to the
 * folder checkout, or null when the file lives outside it (the files pane can
 * only browse the checkout — outside paths stay external-editor territory).
 * Relative paths are trusted to be checkout-relative: the pane's shell starts
 * in the folder dir, and that's also how agents print them.
 */
export function filesPanePathFor(folderDir: string, path: string): string | null {
  if (path.startsWith(`${folderDir}/`)) return path.slice(folderDir.length + 1);
  if (path.startsWith("/") || path.startsWith("~")) return null;
  let rel = path;
  while (rel.startsWith("./")) rel = rel.slice(2);
  if (rel === "" || rel.startsWith("../")) return null;
  return rel;
}

/** The (per-folder) pane id of the folder's live-preview pane — the task's own
 * dev server embedded beside its terminals, with draw-on-page feedback. */
export function previewPaneId(folderDir: string): string {
  return `${PREVIEW_PANE_PREFIX}${folderDir}`;
}

export function isPreviewPane(paneId: string): boolean {
  return paneId.startsWith(PREVIEW_PANE_PREFIX);
}

/** The folder dir a preview pane id points at (null otherwise). */
export function previewPaneDir(paneId: string): string | null {
  return isPreviewPane(paneId) ? paneId.slice(PREVIEW_PANE_PREFIX.length) : null;
}

/** The folder dir any sentinel pane id (diff, files, or preview) points at —
 * null for session and exit panes. This is the single gate `hydrateWins`/
 * `pruneWins` use to keep a folder-derivable pane across restore, so adding a
 * kind here makes it first-class in persistence automatically. */
export function folderPaneDir(paneId: string): string | null {
  return diffPaneDir(paneId) ?? filesPaneDir(paneId) ?? previewPaneDir(paneId);
}

/** The pane id of a crashed session's tombstone. A shell that dies on its own
 * swaps its session pane for this one, so the *layout* records that the task
 * now holds a report rather than a terminal — the alternative, leaving the
 * session id in place and remembering "but it's dead" beside the layout, makes
 * every reader of `panes` responsible for a distinction the id could carry. */
export function exitPaneId(sessionId: string): string {
  return `${EXIT_PANE_PREFIX}${sessionId}`;
}

export function isExitPane(paneId: string): boolean {
  return paneId.startsWith(EXIT_PANE_PREFIX);
}

/** The session a tombstone reports on (null for any other pane). */
export function exitPaneSession(paneId: string): string | null {
  return isExitPane(paneId) ? paneId.slice(EXIT_PANE_PREFIX.length) : null;
}

/** The session a pane belongs to, live or dead — a session pane is its own id,
 * a tombstone unwraps to the session it reports on, folder panes have none.
 * Lets prune/validity rules treat a crashed pane as the session it still is. */
export function paneSession(paneId: string): string | null {
  if (folderPaneDir(paneId) !== null) return null;
  return exitPaneSession(paneId) ?? paneId;
}

// --- Pure window-layout reducers (unit-tested; the screen wraps them in
// `updateWins` for persistence) ---

/** Last id handed out, so a same-millisecond mint can't repeat one. */
let lastWindowSeq = 0;

/**
 * Mint a window id. Every mint site must use this rather than `Date.now()`
 * directly: window ids key `activeWindows`, so two windows sharing one id make
 * both their folders resolve to the same window and only one folder's panes
 * ever mount.
 *
 * A bare millisecond timestamp is not unique enough — restoring several panes
 * across folders after a crash mints them all in one tick (see the resume
 * picker). Keeping the counter monotonic preserves the newest-last ordering
 * the timestamp gave us, while guaranteeing distinctness.
 */
export function nextWindowId(): string {
  const now = Date.now();
  lastWindowSeq = now > lastWindowSeq ? now : lastWindowSeq + 1;
  return `w${lastWindowSeq}`;
}

let openFileNonce = 0;

/**
 * Monotonic re-trigger token for the code viewer's "open this file at this
 * anchor" effect. Same reasoning as {@link nextWindowId}: a `Date.now()` nonce
 * repeats when two opens land in one millisecond, and a repeated nonce reads
 * as "nothing changed", so the second open never scrolls.
 */
export function nextOpenFileNonce(): number {
  return ++openFileNonce;
}

/** Last id handed out, so a same-millisecond mint can't repeat one. */
let lastDraftScopeSeq = 0;

/**
 * Mint a scope id for a new-task form's image staging directory. Same
 * reasoning as {@link nextWindowId}: two forms opened in the same millisecond
 * (e.g. across two repos) would otherwise share a staging dir and clobber
 * each other's pasted images.
 */
export function nextDraftScopeId(): string {
  const now = Date.now();
  lastDraftScopeSeq = now > lastDraftScopeSeq ? now : lastDraftScopeSeq + 1;
  return `draft-${lastDraftScopeSeq}`;
}

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
  let windowId = w.activeWindows[folderDir];
  if (!w.windows.some((win) => win.id === windowId && win.folderDir === folderDir)) {
    // Stale/missing active entry: reuse the folder's first existing window
    // before minting a new one — otherwise a dangling entry spawns a duplicate
    // "primary" beside the window the user already has.
    const existing = w.windows.find((win) => win.folderDir === folderDir);
    if (!existing) {
      // Windows are born around their first pane — never empty.
      const id = newWindowId();
      return {
        windows: [...w.windows, { id, name: "primary", folderDir, panes: [paneId] }],
        activeWindows: { ...w.activeWindows, [folderDir]: id },
      };
    }
    windowId = existing.id;
  }
  return {
    windows: w.windows.map((win) =>
      win.id === windowId ? { ...win, panes: appendPane(win.panes, paneId) } : win,
    ),
    activeWindows: { ...w.activeWindows, [folderDir]: windowId },
  };
}

/** Append while keeping the non-empty tuple type (a plain spread widens to
 * `string[]`). */
function appendPane(panes: Panes, paneId: string): Panes {
  const [first, ...rest] = panes;
  return [first, ...rest, paneId];
}

/** Drop a pane from the window that holds it (pane ids are unique — session
 * ids globally, diff ids per folder — so at most one window matches). A window
 * is a tiling of at least one pane, never an empty container: when the pane
 * was the window's last, the window goes with it, the folder's active window
 * moves to a sibling (or unsets — `placePane` mints a fresh "primary" lazily
 * when the folder next opens a pane). */
export function dropPane(w: WindowsPayload, paneId: string): WindowsPayload {
  const host = w.windows.find((win) => win.panes.includes(paneId));
  if (!host) return w;
  const remaining = toPanes(host.panes.filter((p) => p !== paneId));
  if (!remaining) {
    const sibling = w.windows.find((win) => win.folderDir === host.folderDir && win.id !== host.id);
    const activeWindows = { ...w.activeWindows };
    if (activeWindows[host.folderDir] === host.id) {
      if (sibling) activeWindows[host.folderDir] = sibling.id;
      else delete activeWindows[host.folderDir];
    }
    return { windows: w.windows.filter((win) => win.id !== host.id), activeWindows };
  }
  return {
    ...w,
    windows: w.windows.map((win) => (win.id === host.id ? { ...win, panes: remaining } : win)),
  };
}

/** Swap one pane id for another in place — same window, same position, same
 * column widths. This is how a session pane becomes its own tombstone when the
 * shell crashes (and back again when the session is reopened): the task is the
 * same task, only what fills it changed, so nothing about the tiling should
 * move. Returns `w` untouched when `fromId` isn't in the layout. */
export function replacePane(w: WindowsPayload, fromId: string, toId: string): WindowsPayload {
  const host = w.windows.find((win) => win.panes.includes(fromId));
  if (!host) return w;
  const swap = (p: string) => (p === fromId ? toId : p);
  return {
    ...w,
    windows: w.windows.map((win) => {
      if (win.id !== host.id) return win;
      // Rebuilt head-first so the non-empty tuple type survives the map.
      const [first, ...rest] = win.panes;
      return { ...win, panes: [swap(first), ...rest.map(swap)] };
    }),
  };
}

/** The folder dirs whose slice of the layout (their windows, in order, or
 * their active-window entry) differs between two payloads — exactly the
 * `touchedFolders` the backend's merge-by-folder save needs. Accepts the wire
 * shape so hydration can diff the raw blob against its parsed form. */
export function changedFolderDirs(a: WireWindowsPayload, b: WireWindowsPayload): string[] {
  const dirs = new Set<string>([
    ...a.windows.map((win) => win.folderDir),
    ...b.windows.map((win) => win.folderDir),
    ...Object.keys(a.activeWindows),
    ...Object.keys(b.activeWindows),
  ]);
  return [...dirs].filter((d) => folderSignature(a, d) !== folderSignature(b, d));
}

/** One folder's slice of a layout, as a comparable string. */
function folderSignature(p: WireWindowsPayload, dir: string): string {
  return JSON.stringify([
    p.windows.filter((win) => win.folderDir === dir),
    p.activeWindows[dir] ?? null,
  ]);
}

/** Parse a persisted layout into the live shape. Only folder (diff/files)
 * panes survive a restart, because only they can be rebuilt from what's on
 * disk: a session pane's PTY died with the app and nothing restarts it, and a
 * tombstone reports a crash from a run that's over. Both would restore as
 * tiles with nothing behind them, so both are dropped here — the parse
 * boundary is where "what a blob claims" becomes "what this run can show".
 * Windows left paneless by that (and by old blobs written before empty windows
 * became unrepresentable) are swept — windows are minted lazily, so nothing of
 * value is lost — along with dangling active-window entries. */
export function hydrateWins(w: WireWindowsPayload): WindowsPayload {
  const windows: AgWindow[] = [];
  for (const win of w.windows) {
    const panes = toPanes(win.panes.filter((p) => folderPaneDir(p) !== null));
    if (panes) windows.push({ ...win, panes });
  }
  return normalizeWins({ windows, activeWindows: w.activeWindows });
}

/** Reconcile the persisted layout against what actually exists. The blob on
 * disk outlives its panes: sessions get removed by another app instance, a
 * repo comes off the rail with non-live session records, a crash beats the
 * debounced save — leaving ghost pane ids that hold a tile task with nothing
 * in it (so a fresh pane lands in spot two behind a blank).
 *
 * Drops windows of folders not in `validFolderDirs`, then panes that are
 * neither a known session id nor a valid folder's diff/files pane. A window emptied
 * by this prune vanishes like a closed-out window (`dropPane`'s rule — the
 * empty-pane state is unrepresentable); `placePane` mints a fresh "primary"
 * lazily when the folder next opens a pane. Returns `w` itself when nothing
 * changed, so callers can cheaply skip the save. */
export function pruneWins(
  w: WindowsPayload,
  validSessionIds: ReadonlySet<string>,
  validFolderDirs: ReadonlySet<string>,
): WindowsPayload {
  const kept: AgWindow[] = [];
  for (const win of w.windows) {
    if (!validFolderDirs.has(win.folderDir)) continue;
    const panes = toPanes(
      win.panes.filter((p) => {
        const dir = folderPaneDir(p);
        if (dir !== null) return validFolderDirs.has(dir);
        // A tombstone lives and dies with the session it reports on: once the
        // record is gone there's nothing left to name, so it prunes like the
        // session pane it replaced.
        return validSessionIds.has(paneSession(p)!);
      }),
    );
    if (!panes) continue;
    kept.push(panes.length === win.panes.length ? win : { ...win, panes });
  }
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

/** A compact "waiting 12m" age label for a needing row, from its
 * `needsSinceMs` stamp (the backend holds it stable across snapshots). Returns
 * `null` when the session isn't currently needing you or the stamp is in the
 * future, so callers can render nothing. Rounds like `fmtAge`: sub-minute →
 * "waiting <1m", then minutes / hours / days. */
export function fmtWaitingAge(sinceMs: number | null | undefined, now: number): string | null {
  if (sinceMs == null) return null;
  const diff = now - sinceMs;
  if (diff < 0) return null;
  const mins = Math.floor(diff / 60_000);
  if (mins < 1) return "waiting <1m";
  if (mins < 60) return `waiting ${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `waiting ${hrs}h`;
  return `waiting ${Math.floor(hrs / 24)}d`;
}

/** Every session that currently needs you (`sessionNeeds`), board-wide, ordered
 * oldest-first by `needsSinceMs` so the longest-blocked agent leads the
 * attention feed. Sessions with no stamp (older snapshots) sort last; the sort
 * is stable, so equal ages keep repo→folder→session render order. */
export function needingSessionsOldestFirst(repos: RepoData[]): SessionData[] {
  const needing: SessionData[] = [];
  for (const r of repos)
    for (const f of r.folders)
      for (const s of f.sessions) {
        if (sessionNeeds(s)) needing.push(s);
      }
  return needing
    .map((s, i) => ({ s, i }))
    .toSorted(
      (a, b) => (a.s.needsSinceMs ?? Infinity) - (b.s.needsSinceMs ?? Infinity) || a.i - b.i,
    )
    .map(({ s }) => s);
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

  const targetIndexes = all.map((s, i) => (sessionCatchesEye(s) ? i : -1)).filter((i) => i !== -1);
  if (targetIndexes.length === 0) return null;

  const fromIndex = fromSessionId ? all.findIndex((s) => s.id === fromSessionId) : -1;

  const chosen =
    direction === "next"
      ? (targetIndexes.find((i) => i > fromIndex) ?? targetIndexes[0])
      : ([...targetIndexes].toReversed().find((i) => i < fromIndex) ??
        targetIndexes[targetIndexes.length - 1]);

  return all[chosen];
}

/** A folder's currently-running (PTY-live) sessions. */
export function liveSessions(folder: FolderData): SessionData[] {
  return folder.sessions.filter((s) => s.live);
}

/** Every distinct port-drift entry across a folder's live sessions, deduped
 * by key + spawned/current pair (several panes spawned at different times
 * can carry the same drift, or genuinely different ones if a port rotated
 * more than once) — the detail list for the folder-header badge's tooltip. */
export function folderPortDrift(folder: Pick<FolderData, "sessions">): PortDrift[] {
  const seen = new Map<string, PortDrift>();
  for (const s of folder.sessions) {
    for (const d of s.portDrift ?? []) {
      seen.set(`${d.key}:${d.spawnedPort}:${d.currentPort}`, d);
    }
  }
  return [...seen.values()];
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
 * finished-but-unseen turn) with nothing currently *running*.
 *
 * `f.quiet` (persisted per folder — see `RepoMenu`'s "Mark quiet" action)
 * short-circuits straight to quiet, for a folder the user wants off the rail
 * even while it's technically busy (e.g. mid-review, waiting on someone
 * else). It's one-directional: there's no override in the other direction —
 * a folder is never forced to read as *busy*. */
export function isFolderQuiet(f: FolderData, now: number): boolean {
  return (
    f.quiet ||
    (liveSessions(f).length === 0 &&
      f.filesChanged === 0 &&
      f.commitsAhead === 0 &&
      f.sessions.every((s) => !sessionCatchesEye(s)) &&
      now - folderLastActivityAt(f) >= QUIET_GRACE_MS)
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
/**
 * Parse a git origin URL to its GitHub `owner/name`, or `undefined` when it
 * doesn't look like one. Handles the three shapes that show up in practice —
 * `https://github.com/owner/repo(.git)`, `git@github.com:owner/repo.git`,
 * and `ssh://git@github.com/owner/repo` — by taking the last two path
 * segments and stripping a `.git` suffix. Used to stamp a task's task
 * binding with the repo identity PR auto-attach matches on (#339).
 */
export function ownerRepoFromOrigin(originUrl: string | null | undefined): string | undefined {
  if (!originUrl) return undefined;
  const match = originUrl.trim().match(/[:/]([\w.-]+)\/([\w.-]+?)(?:\.git)?\/?$/);
  if (!match) return undefined;
  return `${match[1]}/${match[2]}`;
}

export function prForFolder(
  prs: PrItem[],
  originUrl: string | null | undefined,
  branch: string,
): PrItem | undefined {
  if (!branch) return undefined;
  const origin = originUrl?.toLowerCase();
  return prs.find((p) => p.branch === branch && (!origin || origin.includes(p.repo.toLowerCase())));
}

/** True when a folder is provably safe to delete: no uncommitted changes
 * (`dirty`) and every commit on this branch has landed on `comparedBase`
 * (`commitsUnlanded === 0`). Deliberately independent of any PR's state —
 * it's a pure git fact — so call sites that want to gate on "the work
 * landed" do that themselves (see `folderLandedButHasWork`). Note this checks a
 * narrower, more optimistic thing than `tt task rm`'s own removal guard
 * (`crates/tt-tasks/src/guards.rs`): that guard only blocks on a dirty tree
 * or commits unreachable from *any* branch/remote (deleting a worktree never
 * deletes its branch, so an unmerged-but-pushed branch is still "safe" by
 * its math). This is the stricter "nothing left to do here" signal — the
 * guard remains the last line of defense either way.
 *
 * `landed` is deliberately *not* part of this. The two answer different
 * questions: this one is "would removing this lose anything", `landed` is
 * "how did the work get to the base". A branch nobody has committed to is
 * safe to delete while never having landed, and a branch whose remote was
 * deleted (`"upstream gone"`) reports landed on the weakest possible evidence
 * while its commits are still counted as outstanding — so requiring `landed`
 * here would be both too strict and too loose.
 *
 * This is *not* the badge gate — it is one half of it. The affirmative
 * "safe to delete" claim additionally requires a merged PR; see
 * {@link folderSafeToDelete}. */
export function folderHoldsNoWork(folder: Pick<FolderData, "dirty" | "commitsUnlanded">): boolean {
  return !folder.dirty && folder.commitsUnlanded === 0;
}

/** Whether this checkout may be shown as **safe to delete**: its PR merged,
 * and nothing here would be lost.
 *
 * Both halves are required, and a folder with **no merged PR is never safe to
 * delete** no matter what git says. Git can prove a branch's *content* reached
 * the base, but it cannot tell "this work landed" apart from "this work was
 * abandoned" — a branch deleted unmerged, or reset away, leaves the same trace
 * as one that shipped. A merged PR is the durable external fact that closes
 * that gap, so the affirmative claim rests on it.
 *
 * The cost is deliberate: a PR-less scratch task never earns the badge, even
 * when git can see it is clean. That is fail-safe — deletion is still offered
 * through the guarded modal — and it is the direction chosen over the looser
 * git-only gate that shipped in #371.
 *
 * Note this is *stricter* than `tt task rm`'s own guard
 * (`crates/tt-tasks/src/guards.rs`), which blocks only on a dirty tree or
 * commits reachable from no branch/remote. Absence of this badge therefore
 * says nothing about whether removal would be refused — the guard remains the
 * last line of defense either way. */
export function folderSafeToDelete(
  folder: Pick<FolderData, "dirty" | "commitsUnlanded">,
  pr: Pick<PrItem, "state"> | undefined,
): boolean {
  return pr?.state === "merged" && folderHoldsNoWork(folder);
}

/** True when this branch's work reached the base branch — proven either by
 * git itself (`folder.landed`, which sees merge commits, rebases and squash
 * merges alike) or by a merged GitHub PR.
 *
 * Both halves are needed. Git can't see a PR that merged into a base this
 * checkout never fetched, and GitHub can't see a branch merged locally or one
 * that never had a PR at all — which used to make every PR-less task read as
 * unfinished forever. Says nothing about whether the checkout still holds
 * work; that's `folderSafeToDelete`. */
export function folderLanded(
  folder: Pick<FolderData, "landed">,
  pr: Pick<PrItem, "state"> | undefined,
): boolean {
  return folder.landed !== null || pr?.state === "merged";
}

/** Whether the delete-worktree affordances (rail menu, the `ab-remove-task`
 * chord) apply to a folder: a worktree that still exists on disk. The
 * main checkout has no `task_remove` path, and a ghost (`dirMissing`) has
 * nothing on disk to delete — its affordance is Untrack. Unrelated to
 * `folderSafeToDelete`: this gates whether deletion can be *offered*, not
 * whether it would succeed (the guarded removal decides that after the
 * confirm). */
export function folderRemovableTask(
  folder: Pick<FolderData, "isWorktree" | "dirMissing">,
): boolean {
  return folder.isWorktree && !folder.dirMissing;
}

/** The guard kinds `RmBlocked::kind()` emits (`crates/tt-tasks/src/guards.rs`).
 * A blocker whose kind isn't one of these is a backend that grew a guard this
 * frontend hasn't been taught yet — rendered generically rather than
 * mislabeled, see `BlockerIcon`. */
export const TASK_BLOCKER_KINDS = ["dirtyTree", "unreachableCommits", "foreignPort"] as const;
export type TaskBlockerKind = (typeof TASK_BLOCKER_KINDS)[number];

/** One reason `task_remove` refused. Blocked is an `Ok` outcome there, not
 * an error — an expected answer with a next step attached — so the UI gets
 * typed rows to act on instead of a newline-joined error string.
 *
 * Inferred from the Zod schema that actually parses the IPC payload, so the
 * validated shape and the type can't drift apart; the field-level rationale
 * (why `kind` stays an open string, what `port` feeds) lives on the schema. */
export type TaskBlocker = z.infer<typeof TaskBlockerSchema>;

/** The `task_remove` result: removed, or refused with reasons. */
export type TaskRemoveOutcome = z.infer<typeof TaskRemoveOutcomeSchema>;

/** What forcing past a blocker would discard, as a noun for the button. */
const DISCARDED: Record<string, string> = {
  dirtyTree: "changes",
  unreachableCommits: "commits",
};

/** Label for the force button in the blocked-delete dialog.
 *
 * The consequence goes *in the button*, not only in the body text: the user
 * already confirmed one dialog that promised the delete was guarded, so this
 * click is where consent to lose work is actually given, and it's the last
 * thing read before committing. When nothing would be lost (a stray listener
 * is the only blocker — forcing orphans a process, it doesn't destroy work)
 * the button says so rather than borrowing a scarier word than it earns.
 *
 * Names only the kinds it recognizes: an unfamiliar `losesWork` kind falls
 * back to the unspecific label rather than asserting it's discarding
 * "commits" because that was the last branch of a ternary. */
export function forceDeleteLabel(blockers: TaskBlocker[]): string {
  const nouns = blockers
    .filter((b) => b.losesWork)
    .map((b) => DISCARDED[b.kind])
    .filter((noun): noun is string => noun !== undefined);
  const unique = [...new Set(nouns)];
  return unique.length > 0 ? `Delete and discard the ${unique.join(" and ")}` : "Delete anyway";
}

/** The port this blocker offers to clear, or `null` when there's nothing to
 * act on. Only a `foreignPort` blocker is something `task_stop_port` will
 * touch, and only if its number survived — a port blocker without one still
 * renders its remedy as text, it just gets no button. */
export function stoppablePort(blocker: TaskBlocker): number | null {
  if (blocker.kind !== "foreignPort") return null;
  return typeof blocker.port === "number" ? blocker.port : null;
}

/** True when a branch having landed doesn't actually make its checkout safe to
 * delete: the work reached the base (`folderLanded` — a merged PR, or git's
 * own proof), but the checkout still has uncommitted changes or commits that
 * haven't landed yet (`!folderHoldsNoWork`).
 *
 * This is the "squash-merged, but 2 uncommitted files" case, and it is exactly
 * where a merged badge would otherwise lie. Replaces the old PR-only check:
 * that one couldn't warn about a task whose branch merged locally, which is
 * the same data-loss risk with no GitHub row to notice it.
 *
 * Pairs with `folderHoldsNoWork`, not `folderSafeToDelete`: this warns that
 * finished-looking work is still held, which is true whether or not a PR
 * merged. Gating it on a merged PR would silence the warning for exactly the
 * locally-merged branch it was added to cover. */
export function folderLandedButHasWork(
  folder: Pick<FolderData, "dirty" | "commitsUnlanded" | "landed">,
  pr: Pick<PrItem, "state"> | undefined,
): boolean {
  return folderLanded(folder, pr) && !folderHoldsNoWork(folder);
}

/** A per-folder actionable signal: the checkout is safe to clean up, has
 * sessions waiting on you, or its live panes have drifted ports. Each is
 * already a badge somewhere in the rail (`SafeToDeleteBadge`, `NeedsBadge`,
 * `PortDriftBadge`) — this is the same data as a flat list, for the
 * working-context band (more room than a rail row) to spell each one out
 * instead of a glyph. */
export type ActionableKind = "safe-to-delete" | "needs-you" | "port-drift";

export type ActionableItem = {
  kind: ActionableKind;
  subtitle: string;
  /** Set only for `safe-to-delete` — the merged PR being cleaned up. */
  pr?: Pick<PrItem, "number" | "url">;
};

/** Every actionable signal that applies to one folder — the same gates the
 * rail's badges use (`folderSafeToDelete`, `folder.needs > 0`,
 * `folderPortDrift`). `pr` is the folder's already-resolved PR (see
 * `prForFolder`), not looked up again here.
 *
 * The safe-to-delete signal requires a merged PR, so its subtitle always names
 * one, and adds git's own account of the landing when it has one — the PR says
 * the work was accepted, `landed` says the content is actually in this
 * checkout's base. */
export function folderActionableItems(
  folder: Pick<
    FolderData,
    "isWorktree" | "dirty" | "commitsUnlanded" | "landed" | "comparedBase" | "needs" | "sessions"
  >,
  pr: PrItem | undefined,
): ActionableItem[] {
  const items: ActionableItem[] = [];

  const merged = pr?.state === "merged" ? pr : undefined;
  if (merged && folder.isWorktree && folderSafeToDelete(folder, pr)) {
    const how = folder.landed
      ? `PR #${merged.number} merged, ${folder.landed} into ${comparedBaseLabel(folder)}`
      : `PR #${merged.number} merged`;
    items.push({
      kind: "safe-to-delete",
      subtitle: `${how}, no uncommitted changes, every commit landed`,
      pr: { number: merged.number, url: merged.url },
    });
  }

  if (folder.needs > 0) {
    items.push({
      kind: "needs-you",
      subtitle: `${folder.needs} session${folder.needs === 1 ? "" : "s"} waiting on you`,
    });
  }

  const drift = folderPortDrift(folder);
  if (drift.length > 0) {
    items.push({
      kind: "port-drift",
      subtitle: drift.map((d) => `${d.key} ${d.spawnedPort} → ${d.currentPort}`).join(", "),
    });
  }

  return items;
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

/** Token counts at a glance: `53K`, `412K`, `1M`. Context windows are round
 * numbers and the exact token count is never actionable, so this trades the
 * digits for something readable at 10px. */
export function fmtTokens(n: number): string {
  if (n < 1_000) return `${n}`;
  const k = Math.round(n / 1_000);
  // Promote on the *rounded* value, not the raw one: 999_500 rounds to 1000K,
  // which has to read as 1M.
  if (k < 1_000) return `${k}K`;
  const m = Math.round(n / 100_000) / 10;
  // 1M, not 1.0M — but keep 1.5M's fraction. Tested after rounding, so 1_020_000
  // reads as 1M rather than 1.0M.
  return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
}

/** `412K / 1M` — what a session is holding against what it can hold. Null when
 * the window size is unknown, since a bare used-count answers nothing.
 *
 * The window outlives the counter (a journal rotation clears what's used but
 * not how much fits), so an unknown count reads as `1M window` rather than a
 * `0` we'd be inventing. */
export function fmtContext(d: AgentEventDetails | null | undefined): string | null {
  if (!d?.contextMax) return null;
  if (!d.contextUsed) return `${fmtTokens(d.contextMax)} window`;
  return `${fmtTokens(d.contextUsed)} / ${fmtTokens(d.contextMax)}`;
}

/** `claude-opus-4-8 · 412K / 1M` — what a cold resume would cost, in one line:
 * which model, and how much context it would re-send. Null when neither is
 * known, so a caller can render nothing rather than a stray separator. */
export function modelContextLabel(d: AgentEventDetails | null | undefined): string | null {
  const parts = [d?.model, fmtContext(d)].filter(Boolean);
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** True when a folder's branch label would only restate the folder's own
 * name: a worktree task's directory *is* the slug of its branch
 * (`feat/model-indicator-badge` → `feat-model-indicator-badge`), so printing
 * both spends a rail line saying one thing twice. The slug rules mirror
 * `tt-git`'s `slug()` (`crates/tt-git/src/branch_name.rs`): lowercase, trim,
 * anything outside `[0-9a-z_-]` to `-`, collapse runs, strip trailing `-`.
 * A main checkout (`towles-tool-rs` on `main`) never matches, so it keeps
 * its branch label. */
export function branchRedundant(folderName: string, branch: string | null | undefined): boolean {
  if (!branch) return false;
  const slug = branch
    .toLowerCase()
    .trim()
    .replace(/[^0-9a-z_-]+/g, "-")
    .replace(/-{2,}/g, "-")
    .replace(/-+$/, "");
  return folderName === slug;
}

/** Model family → the single letter the rail's `ModelBadge` shows: `H`aiku,
 * `S`onnet, `O`pus, `F`able, `M`ythos. Matches on the family token inside the
 * id (`claude-opus-4-8` → `O`), deliberately dropping the version — the badge
 * answers "which brain", the tooltip carries the exact id. Null for an unknown
 * family rather than a guessed letter, so the badge simply doesn't render. */
export function modelLetter(model: string | null | undefined): string | null {
  if (!model) return null;
  const family = ["haiku", "sonnet", "opus", "fable", "mythos"].find((f) =>
    model
      .toLowerCase()
      .split(/[-_./:\s]/)
      .includes(f),
  );
  return family ? family[0].toUpperCase() : null;
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
  if (r.busy > 0) return "bg-cyan-500";
  if (r.total > 0) return "bg-emerald-500";
  return null;
}

/** Badge text color to pair with `rollupAlertColor`'s background — white
 * reads fine on the red/blue/emerald fills, but on cyan-500 (busy) it's
 * nearly illegible, so that one badge needs dark text instead. */
export function rollupAlertTextColor(bg: string | null): string {
  return bg === "bg-cyan-500" ? "text-cyan-950" : "text-white";
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

      const { listen } = await import("@tauri-apps/api/event");

      // Every payload is stamped with its compute time (`ts`) — never let an
      // older snapshot replace a newer one. The initial `ab_get_state` fetch
      // below resolves *after* the subscription is live, so a debounced
      // `agentboard://state` event (e.g. the one `ab_add_repo` triggers during
      // task creation) can land first; without the guard the slower fetch
      // would roll the rail back to a snapshot that predates it.
      const accept = (payload: StatePayload) =>
        setState((cur) => (payload.ts < cur.ts ? cur : payload));

      const sub = await listen<StatePayload>("agentboard://state", (e) => {
        accept(e.payload);
      });
      if (disposed) {
        sub();
        return;
      }
      unlisten = sub;

      const initial = await invoke<StatePayload>("ab_get_state");
      if (initial.isOk() && !disposed) accept(initial.value);
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return state;
}

/** Status dot color, mirroring the Rust `AgentStatus::color` intent.
 * `busy` is cyan rather than the more obvious amber/yellow: amber is this
 * app's needs-you accent (`sessionCatchesEye`, folder badges), and
 * yellow-500 sits only ~8° from amber-500 in hue — close enough that a busy
 * (no action needed) dot reads as an attention flag at a glance. Cyan stays
 * unambiguously "working", not "look at this".
 * `interrupted` is orange-800 rather than orange-500 for the same reason:
 * orange-500 sits inside both amber-500's and red-500's confusion radius
 * (OKLab ΔE ~10, under the ~15 floor where even normal color vision starts
 * struggling — checked with the dataviz skill's palette validator) and an
 * unseen interrupted session shows this dot inside an amber-washed
 * needs-you row, i.e. right next to the color it must stay distinct from.
 * orange-800 clears both by a wide margin (ΔE 18–31). */
export function statusColor(status: AgentStatus): string {
  switch (status) {
    case "busy":
      return "bg-cyan-500";
    case "complete":
      return "bg-green-500";
    case "error":
      return "bg-red-500";
    case "waiting":
      return "bg-blue-500";
    case "interrupted":
      return "bg-orange-800";
    default:
      return "bg-muted-foreground/40";
  }
}

// --- Session PTY writes ---

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Write raw bytes into a session's PTY. Fails when the PTY isn't running. */
export const termWrite = (termId: string, data: string) =>
  invoke<void>("term_write", { termId, data });

/** Write, retrying while the PTY spawns (a just-mounted terminal takes a beat
 * before `term_start` registers it). Gives up after ~3s, resolving the last
 * attempt's failure. */
export async function termWriteRetry(
  termId: string,
  data: string,
): Promise<Result<void, IpcError>> {
  let last = await termWrite(termId, data);
  for (let i = 1; i < 20 && last.isErr(); i++) {
    await sleep(150);
    last = await termWrite(termId, data);
  }
  return last;
}

/** Wait for `termId`'s first `terminal://frame` (the shell's first output —
 * a real proxy for "the PTY is actually reading input", unlike a successful
 * `term_write`, which only proves the Rust-side write conduit exists and can
 * still race the shell sourcing its rc files and eating a queued command).
 * Resolves early if the terminal never spawns (falls back to a flat wait). */
export async function waitForFirstFrame(termId: string, timeoutMs = 5000): Promise<void> {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { listen } = await import("@tauri-apps/api/event");
  await new Promise<void>((resolve) => {
    let settled = false;
    let unlisten: (() => void) | undefined;
    const timer = setTimeout(finish, timeoutMs);
    function finish() {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      unlisten?.();
      resolve();
    }
    listen<{ termId: string }>("terminal://frame", (e) => {
      if (e.payload.termId === termId) finish();
    }).then((u) => (settled ? u() : (unlisten = u)));
  });
}

/** Single-quote a string for safe injection into a shell command line typed
 * into a PTY (POSIX `'...'` escaping — embedded `'` becomes `'\''`). */
export function shellQuote(text: string): string {
  return `'${text.replace(/'/g, `'\\''`)}'`;
}

/** `claude --model` aliases for the latest model generation (see `claude --help`). */
export type ClaudeModel = "sonnet" | "opus" | "fable";

/** `claude --effort` levels (see `claude --help`). */
export type ClaudeEffort = "low" | "medium" | "high" | "xhigh" | "max";

export type ClaudeLaunchOptions = {
  model?: ClaudeModel;
  effort?: ClaudeEffort;
  /** Start the session in a specific permission mode (`claude
   * --permission-mode`). The dynamic-task flow launches in `plan` so the
   * session explores and presents a plan before any edit is possible. */
  permissionMode?: "plan";
};

/** The `claude` invocation for a session's PTY: bare, or with an initial
 * prompt passed as an argument so Claude starts working on it immediately
 * instead of waiting at an empty prompt, plus an optional `--model`/`--effort`
 * pair for callers (e.g. the new-task dialog) that let the user pick both. */
export function claudeCommand(prompt: string, options?: ClaudeLaunchOptions): string {
  const trimmed = prompt.trim();
  const parts = [
    "claude",
    options?.model ? `--model ${shellQuote(options.model)}` : null,
    options?.effort ? `--effort ${shellQuote(options.effort)}` : null,
    options?.permissionMode ? `--permission-mode ${shellQuote(options.permissionMode)}` : null,
    trimmed ? shellQuote(trimmed) : null,
  ].filter((p): p is string => p != null);
  return `${parts.join(" ")}\r`;
}

/** Wrap a dynamic task's goal with the delivery pipeline the session runs
 * once its plan is approved: implement → `/code-review low --fix` →
 * `/simplify` → rebase onto the base branch → PR → merge. The session is
 * launched in plan mode (see `ClaudeLaunchOptions.permissionMode`), so "the
 * plan is approved" is the user's interactive approval in the PTY — after
 * that gate the instructions carry the session all the way to a merged PR,
 * and the merged PR is what rolls the board task to done (PR auto-attach +
 * status rollup on collect).
 *
 * `base` should be the *effective* base ref (`TaskCreated.baseLabel`, e.g.
 * `origin/main`), not the local branch name: inside the task's worktree a
 * fetch never advances the local base ref, so "rebase onto main" would mean
 * stale history. */
export function dynamicFlowPrompt(goal: string, base: string): string {
  const trimmed = goal.trim();
  // Single line by construction — like `promptWithImages`, this is typed into
  // a PTY inside a quoted arg, where a literal newline drops zsh to PS2.
  return [
    trimmed ? `${trimmed} — ` : "",
    "This is a dynamic task: after your plan is approved, deliver it all the way ",
    "to a merged PR without stopping to ask. You are in a dedicated worktree on ",
    `this task's branch; the target branch is ${base}. Once the plan is approved: `,
    "(1) implement it, verifying with the project's build/lint/test commands, and ",
    "commit; (2) run /code-review low --fix; (3) run /simplify; commit what those ",
    `two change; (4) fetch and rebase this branch onto the latest ${base}, `,
    "resolving conflicts; (5) push and open the PR with gh pr create; (6) merge it ",
    "with gh pr merge, using a strategy the repo allows — but if the merge is ",
    "blocked by required checks or reviews you cannot satisfy, stop and report ",
    "instead of forcing it. Then stop: leave the worktree in place — the board ",
    "task closes itself once the merged PR is detected.",
  ].join("");
}

/** MIME types the new-task form accepts off the clipboard — the same closed
 * set `tt_tasks::pasted` writes an extension for. Filtering here means an
 * unsupported paste is ignored at the point of paste (where the user can see
 * it didn't take) rather than erroring minutes later mid-create. */
const PASTEABLE_IMAGE_MIMES = ["image/png", "image/jpeg", "image/jpg", "image/gif", "image/webp"];

export function isPasteableImage(mime: string): boolean {
  return PASTEABLE_IMAGE_MIMES.includes(mime.split(";")[0].trim().toLowerCase());
}

/** Mirrors `MAX_IMAGE_BYTES` in `tt_tasks::pasted`. Checked here as well as
 * there so an over-cap paste is refused while the form is still open — the
 * Rust-side check is the backstop, but by the time it fires the task already
 * exists and the error is much less actionable. */
export const MAX_PASTED_IMAGE_BYTES = 10 * 1024 * 1024;

/** An image sitting in the new-task form, waiting for its task to exist.
 * `dataBase64` is what crosses to Rust; `previewUrl` is the same bytes as a
 * data URL, kept for the thumbnail (a data URL rather than an object URL so
 * there's no lifetime to manage against React's re-renders). */
export type PastedImage = {
  id: string;
  name: string;
  mime: string;
  dataBase64: string;
  previewUrl: string;
};

/** Pull every image off a paste/drop's `DataTransfer`, decoded to base64.
 * Returns `[]` for a plain-text paste, which the caller treats as "not an
 * image paste, let the textarea handle it normally". */
export async function imagesFromDataTransfer(data: DataTransfer | null): Promise<PastedImage[]> {
  const files = Array.from(data?.items ?? [])
    .filter((it) => it.kind === "file" && isPasteableImage(it.type))
    .map((it) => it.getAsFile())
    .filter((f): f is File => f != null);
  const tooBig = files.find((f) => f.size > MAX_PASTED_IMAGE_BYTES);
  if (tooBig) {
    throw new Error(
      `${tooBig.name || "that image"} is ${Math.round(tooBig.size / 1024 / 1024)}MB — over the ${
        MAX_PASTED_IMAGE_BYTES / 1024 / 1024
      }MB limit for an attached image.`,
    );
  }
  return Promise.all(files.map((file, i) => readImageFile(file, i)));
}

/** Ask Rust for the system clipboard's image, for the case where the paste
 * event gave us nothing.
 *
 * A WebKitGTK paste event carries no image data at all — on Linux, Ctrl+V of
 * a screenshot fires `paste` with empty `clipboardData`, so there is nothing
 * in the DOM event to read. `read_clipboard_image` goes to the OS clipboard
 * directly; `null` means it holds no image, which is a normal outcome — as does
 * a failed read, since there's nothing to attach either way. */
export async function clipboardImageFromHost(): Promise<PastedImage | null> {
  const result = await invoke<{ mime: string; dataBase64: string } | null>("read_clipboard_image");
  const image = result.unwrapOr(null);
  if (!image) return null;
  return {
    id: `clipboard-${image.dataBase64.length}`,
    name: "clipboard image",
    mime: image.mime,
    dataBase64: image.dataBase64,
    previewUrl: `data:${image.mime};base64,${image.dataBase64}`,
  };
}

async function readImageFile(file: File, index: number): Promise<PastedImage> {
  const previewUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.addEventListener("load", () => resolve(String(reader.result)));
    reader.addEventListener("error", () =>
      reject(reader.error ?? new Error("couldn't read the pasted image")),
    );
    reader.readAsDataURL(file);
  });
  return {
    // Clipboard images arrive as `image.png` every time, so the index keeps
    // React keys distinct across several pastes in one form.
    id: `${file.name || "pasted"}-${index}-${previewUrl.length}-${file.size}`,
    name: file.name || `pasted-image-${index + 1}`,
    mime: file.type,
    dataBase64: previewUrl.slice(previewUrl.indexOf(",") + 1),
    previewUrl,
  };
}

/** Fold images pasted into the new-task form into that task's opening prompt.
 *
 * The prompt crosses into Claude as a single argv string (`claudeCommand`),
 * so an image rides along as a *path* — `task_write_pasted_images` has
 * already written the bytes inside the task, and Claude's Read tool loads an
 * image from a path. The wording names the files and says to read them
 * first, because a bare path in a prompt is something Claude may or may not
 * act on; an explicit instruction is what makes the attachment reliable
 * rather than incidental.
 *
 * A goal-less paste is still a valid prompt — the image alone is the ask.
 *
 * Deliberately newline-free: this string is typed into a live PTY inside a
 * single-quoted argument, and a literal newline mid-quote makes zsh's line
 * editor accept the partial line and drop to a PS2 continuation prompt. It
 * recovers, but there's no reason to add that fragility for formatting. */
export function promptWithImages(goal: string, imagePaths: string[]): string {
  const trimmed = goal.trim();
  if (imagePaths.length === 0) return trimmed;
  const one = imagePaths.length === 1;
  const attachment = `Attached ${one ? "image" : "images"} — read ${
    one ? "it" : "them"
  } first, before anything else: ${imagePaths.join(" ")}`;
  return trimmed ? `${trimmed} — ${attachment}` : attachment;
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
 * session there. The failure stays in the `Result` for the caller to surface. */
export const abOpenSessionForCwd = (cwd: string) =>
  invoke<OpenedSession>("ab_open_session_for_cwd", { cwd }, { schema: OpenedSessionSchema });

/** A cross-screen handoff: "select this folder/session in Agentboard, then
 * type a resume command into it." Agentboard may not be mounted yet when the
 * request is made (e.g. the Claude Sessions screen is the active tab), so
 * this can't be a plain function call — it's a one-shot mailbox: `requestOpenSession`
 * either delivers immediately (a listener is already mounted) or stashes the
 * request for Agentboard's mount effect to consume via `consumePendingOpenSessions`. */
export type PendingOpenSession = {
  folderDir: string;
  sessionId: string;
  resumeId: string;
  label: string;
};

/** A queue, not a single task: the crash-resume picker hands off every ticked
 * pane at once, at boot, when Agentboard is typically not mounted yet — so all
 * of them stash and keeping only the last would resume one session out of N. */
let pendingOpenSessions: PendingOpenSession[] = [];
const openSessionListeners = new Set<(req: PendingOpenSession) => void>();

export function requestOpenSession(req: PendingOpenSession) {
  if (openSessionListeners.size > 0) {
    for (const l of openSessionListeners) l(req);
    return;
  }
  pendingOpenSessions.push(req);
}

export function consumePendingOpenSessions(): PendingOpenSession[] {
  const reqs = pendingOpenSessions;
  pendingOpenSessions = [];
  return reqs;
}

export function onOpenSessionRequest(cb: (req: PendingOpenSession) => void): () => void {
  openSessionListeners.add(cb);
  return () => openSessionListeners.delete(cb);
}

/**
 * One pane the app was running Claude in when it last died. Mirrors the Rust
 * `ResumeCandidate` payload (`tt_agentboard::resume`).
 */
export type ResumeCandidate = {
  folderDir: string;
  /** tt's PTY session id — the pane to restore into. */
  paneId: string;
  paneName: string;
  /** The thread id to hand to `claude --resume`. */
  claudeSessionId: string;
  title: string | null;
  /** Transcript mtime: when this session was last worked on. */
  lastActiveMs: number;
};

/**
 * Panes to offer resuming after an unexpected exit. Empty unless the previous
 * run actually crashed — the backend returns nothing after a clean shutdown,
 * and only answers once per launch, so this is safe to call unconditionally on
 * startup and it will not re-prompt on a webview reload.
 */
export const resumeCandidates = () => invoke<ResumeCandidate[]>("ab_resume_candidates");

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
  /** Start a `.claude/launch.json` dev-server config in a fresh session in
   * `folderDir` — same PTY-typing path as `startClaude`. */
  launchDevServer: (folderDir: string, cfg: LaunchConfigStatus) => void;
  /** Mount + select the pane a dev-server config already runs in. */
  focusSession: (folderDir: string, sessionId: string) => void;
};

/** Percent-rect for one pane in the active window's tiling: side-by-side up to
 * three across, a 2-column grid from four panes on. */
export type PaneRect = { left: number; top: number; width: number; height: number };

/** Column widths (`AgWindow.cols`) are stored in per-mille of the tiling width
 * so persisted layouts stay integer (the Rust mirror keeps `Eq`). */
export const COL_TOTAL = 1000;
/** Narrowest a column can be dragged, per-mille (10%). */
const MIN_COL = 100;
/** Divider snap targets: thirds and fifths of the tiling width, plus the even
 * split so a drag can land back on the default. Magnetic within
 * `SNAP_THRESHOLD`; outside it the divider moves freely. */
const SNAP_POINTS = [200, 333, 400, 500, 600, 667, 800];
const SNAP_THRESHOLD = 25;

/** How many resizable columns an `n`-pane tiling has: the row of n up to
 * three, then the grid's fixed two. */
export function colCount(n: number): number {
  return n <= 3 ? n : 2;
}

/** `cols` when it matches the current layout (right length, sane values), else
 * `null` — a pane count changed since the drag falls back to equal columns. */
function validCols(n: number, cols: number[] | undefined): number[] | null {
  const k = colCount(n);
  if (!cols || cols.length !== k) return null;
  if (cols.some((c) => !Number.isInteger(c) || c < MIN_COL)) return null;
  return cols.reduce((a, b) => a + b, 0) === COL_TOTAL ? cols : null;
}

/** Equal per-mille split, remainder on the last column (k=3 → 333/333/334). */
function equalCols(k: number): number[] {
  const base = Math.floor(COL_TOTAL / k);
  return Array.from({ length: k }, (_, i) => (i === k - 1 ? COL_TOTAL - base * (k - 1) : base));
}

/** Column widths in percent for an `n`-pane tiling under `cols`. */
function colWidths(n: number, cols: number[] | undefined): number[] {
  const valid = validCols(n, cols);
  // Multiply first: `(c * 100) / 1000` divides integers, so 200‰ → exactly 20.
  if (valid) return valid.map((c) => (c * 100) / COL_TOTAL);
  const k = colCount(n);
  return Array.from({ length: k }, () => 100 / k);
}

export function paneRects(n: number, cols?: number[]): PaneRect[] {
  if (n <= 0) return [];
  const widths = colWidths(n, cols);
  if (n <= 3) {
    let left = 0;
    return widths.map((width) => {
      const r = { left, top: 0, width, height: 100 };
      left += width;
      return r;
    });
  }
  const rows = Math.ceil(n / 2);
  const h = 100 / rows;
  return Array.from({ length: n }, (_, i) => {
    const lastRowSolo = n % 2 === 1 && i === n - 1;
    return {
      left: lastRowSolo || i % 2 === 0 ? 0 : widths[0],
      top: Math.floor(i / 2) * h,
      width: lastRowSolo ? 100 : widths[i % 2],
      height: h,
    };
  });
}

/** Magnetic snap: pull a divider position (per-mille) onto the nearest
 * third/fifth/half when within `SNAP_THRESHOLD`, else keep it (integered). */
export function snapCol(pos: number): number {
  for (const p of SNAP_POINTS) {
    if (Math.abs(pos - p) <= SNAP_THRESHOLD) return p;
  }
  return Math.round(pos);
}

/** Column widths after dragging divider `i` (the boundary between columns `i`
 * and `i+1`) to `pos` per-mille from the tiling's left edge. Snaps via
 * `snapCol`, then clamps so both adjacent columns keep `MIN_COL`; columns not
 * adjacent to the divider are untouched. */
export function dragCol(n: number, cols: number[] | undefined, i: number, pos: number): number[] {
  const widths = [...(validCols(n, cols) ?? equalCols(colCount(n)))];
  if (i < 0 || i >= widths.length - 1) return widths;
  const leftEdge = widths.slice(0, i).reduce((a, b) => a + b, 0);
  const lo = leftEdge + MIN_COL;
  const hi = leftEdge + widths[i] + widths[i + 1] - MIN_COL;
  const target = Math.min(hi, Math.max(lo, snapCol(pos)));
  const pair = widths[i] + widths[i + 1];
  widths[i] = target - leftEdge;
  widths[i + 1] = pair - widths[i];
  return widths;
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

/** A repo row in Settings → Agentboard → Repos (from `ab_discover_repos`): every
 * repo under the scan roots, unioned with every repo already on the rail. */
export type RepoCandidate = { name: string; dir: string; active: boolean };

/** What a repo-remove confirmation (or immediate removal) needs to act on. */
export type RemoveTarget = { label: string; dirs: string[]; sessionIds: string[] };

/** A worktree deletion the removal guards refused. Keeps the original
 * `target` so every remedy in the dialog (stop the port's process, force)
 * retries the same removal — the user shouldn't have to find the row again
 * after resolving what blocked it. */
export type BlockedDelete = {
  target: RemoveTarget;
  name: string;
  blockers: TaskBlocker[];
  /** Caveats about how the verdict was reached (a failed fetch → stale refs),
   * carried through so the dialog can qualify the blockers it lists. */
  messages: string[];
};

/** A session about to get Claude launched in it, awaiting the "what are you
 * working toward?" prompt (see `commitStartClaude`). `restart` runs the
 * interrupt-then-relaunch dance first (a live Claude sits in the shell). */
export type StartClaudeTarget = {
  folderDir: string;
  sessionId: string;
  sessionName: string;
  restart: boolean;
};
