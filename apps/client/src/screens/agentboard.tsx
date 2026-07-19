import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  CalendarClock,
  CircleAlert,
  Eye,
  EyeOff,
  FileDiff,
  FolderGit2,
  FolderPlus,
  FolderX,
  GitCommitHorizontal,
  GitPullRequest,
  Network,
  PanelLeftClose,
  Plus,
  TerminalSquare,
} from "lucide-react";
import { fmtMins, PanePlaceholder } from "@/components/agentboard-bits";
import { ColdCacheOverlay, PaneHeader, WorkingContext } from "@/components/agentboard-pane";
import { RailIconStrip, RepoGroup, RollupChip } from "@/components/agentboard-rail";
import { DiffPane } from "@/components/diff-pane";
import { FolderFilesPane, type FilesOpenRequest } from "@/components/files-pane";
import { PreviewPane } from "@/components/preview-pane";
import {
  type NewSlotRepo,
  type NewTaskSubmit,
  type PendingSlot,
  type SlotCreated,
} from "@/components/inline-new-slot";
import { TerminalView } from "@/components/terminal-view";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SlotCreatedSchema, SlotRemoveOutcomeSchema } from "@/lib/schemas/slots";
import { cn } from "@/lib/utils";
import {
  changedFolderDirs,
  ClaudeLaunchOptions,
  claudeCommand,
  dynamicFlowPrompt,
  claudeResumeCommand,
  claudeTitleName,
  consumePendingAgentboardNav,
  consumePendingOpenSessions,
  cycleNeedsYou,
  COL_TOTAL,
  diffPaneDir,
  diffPaneId,
  dragCol,
  filesPaneDir,
  filesPaneId,
  dropPane,
  exitPaneId,
  exitPaneSession,
  hydrateWins,
  isAgent,
  isExitPane,
  isCacheExpiring,
  isDiffPane,
  isFilesPane,
  isPreviewPane,
  previewPaneDir,
  previewPaneId,
  folderRemovableSlot,
  forceDeleteLabel,
  isFolderQuiet,
  liveSessions,
  normalizeWins,
  onAgentboardNavRequest,
  onOpenSessionRequest,
  paneRects,
  nextOpenFileNonce,
  nextWindowId,
  placePane,
  replacePane,
  prForFolder,
  ownerRepoFromOrigin,
  promptWithImages,
  pruneWins,
  sessionLabel,
  sleep,
  stoppablePort,
  termWriteRetry,
  useAgentboardState,
  useNow,
  waitForFirstFrame,
  type AgentboardNav,
  type AgentStatus,
  type AgWindow,
  type BlockedDelete,
  type FolderData,
  type Overlay,
  type PaneRect,
  type PendingOpenSession,
  type RemoveTarget,
  type RepoData,
  type Selected,
  type SessionActions,
  type SessionData,
  type SlotBlockerKind,
  type SlotRemoveOutcome,
  type StartClaudeTarget,
  type StatePayload,
  type WindowsPayload,
  windowColor,
} from "@/lib/agentboard";
import { errorMessage, NotInTauri } from "@/lib/errors";
import { launchCommand, launchRegister, type LaunchConfigStatus } from "@/lib/launch";
import { exitIsCrash, exitLabel, type TermExit } from "@/lib/term-protocol";
import { invoke, isTauri } from "@/lib/tauri";
import type { OpenFileRequest } from "@/lib/ide";
import { shortcutHint, useShortcuts } from "@/lib/shortcuts";
import {
  fmtCountdown,
  storeAddTask,
  storeAttachTaskIssue,
  storeTaskSetSlot,
  useStoreSnapshot,
} from "@/lib/data";
import { useFocusTarget } from "@/lib/focus-target";
import { railRowMotion } from "@/lib/rail-motion";
import { AnimatePresence, motion } from "motion/react";
import { openExternalUrl } from "@/lib/open-url";
import { useWorkspace } from "@/lib/workspace";
import { untrackRepo } from "@/lib/repo-actions";
import { uiAction } from "@/lib/ui-action";
import { toast } from "sonner";

/**
 * Agentboard — the Folder Rail. Left: rollup tally + needs-you strip + the
 * repos → folders (checkouts) → PTY sessions tree. Right: in-app *windows*,
 * scoped to whichever folder is active (clicking a folder header or a
 * session row focuses it) — each a named tiling of that folder's session
 * panes (side-by-side up to 3, then a 2-col grid), switched via the window
 * strip. A window never holds panes from more than one folder. Clicking a
 * rail session opens it as a pane in its own folder's active window; the
 * colored square on a row is its window's group tag. A session IS a PTY;
 * "agent" (✦) is a badge on a session where Claude is detected running —
 * status is reported, never re-rendered (the real TUI is the PTY). All
 * opened terminals live in one flat mounted pool (hidden unless in the
 * active folder's active window) so scrollback survives switching and
 * regrouping. A folder's diff and its file tree each open as their own pane
 * in the same tiling (never a modal), so you review while the agents keep
 * working. Layout persists via
 * debounced `ab_save_windows`. Shortcuts come from the registry in
 * lib/shortcuts.tsx (⌘D new session, ⌘⇧W close session, ⌘⇧G diff pane,
 * ⌘⇧N/⌘⇧P jump to the next/previous session that needs you — `cycleNeedsYou`
 * in lib/agentboard.ts, board-wide, wraps around — ⌘⇧S add another session
 * as a pane in the active window, skipping straight to it when there's only
 * one candidate and opening a picker otherwise), active only while this tab
 * is shown.
 */
/** Sentinel key in the persisted collapse map for "the whole rail is collapsed
 * to icons" — rides the same `ab_save_collapsed` store as the per-row keys
 * (`repo:<name>` / `<repoKey>::<dir>`), which it can never collide with. */
const RAIL_COLLAPSE_KEY = "__rail__";

/** `onOpenChange` for a dialog whose only close-side effect is clearing
 * whatever state made it open — Radix fires `false` on outside-click, Esc,
 * and the built-in close button alike, so this covers all three at once. */
const closeOnFalse = (fn: () => void) => (isOpen: boolean) => {
  if (!isOpen) fn();
};

/** Untrack every repo whose directory is gone from disk, reporting the count.
 * The Rust side re-probes at call time, so a directory restored since the last
 * poll survives. */
async function cleanupMissing() {
  const removed = await invoke<string[]>("ab_untrack_missing", {});
  if (removed.isErr()) {
    toast.error(`Couldn't clean up — ${removed.error.message}`);
    return;
  }
  const n = removed.value.length;
  toast(n > 0 ? `Untracked ${n} missing repo${n === 1 ? "" : "s"}.` : "Nothing to clean up.");
}

/** A pane's grid rect as absolute-positioning percentages. */
const paneStyle = (r: PaneRect) => ({
  left: `${r.left}%`,
  top: `${r.top}%`,
  width: `${r.width}%`,
  height: `${r.height}%`,
});

/**
 * Create the board task for a new-task submit (#339): the task row exists
 * from the moment of submit — before any worktree work — with the picked
 * issues attached. Best-effort: a store failure must not block the worktree
 * (the slot is still useful without a card), so this resolves to `undefined`
 * on error after surfacing a toast.
 */
async function createTaskForSubmit(input: NewTaskSubmit): Promise<number | undefined> {
  const title = input.goal || input.issues[0]?.title || input.branch;
  if (!title) return undefined;
  const status = input.worktree ? "doing" : "backlog";
  const created = await storeAddTask(title, { status });
  if (created.isErr()) {
    if (!NotInTauri.is(created.error)) {
      toast(`couldn't add the board task: ${created.error.message}`);
    }
    return undefined;
  }
  for (const issue of input.issues) {
    void storeAttachTaskIssue(created.value, issue.repo, issue.number, issue.url);
  }
  return created.value;
}

/** Glyph per blocker kind. Exhaustive over `SlotBlockerKind`, so a guard
 * added in Rust fails the build here rather than silently picking up
 * whichever icon a ternary happened to end on. */
const BLOCKER_ICONS: Record<SlotBlockerKind, typeof FileDiff> = {
  dirtyTree: FileDiff,
  unreachableCommits: GitCommitHorizontal,
  foreignPort: Network,
};

/** Glyph for a removal blocker. Tinted by consequence rather than by kind:
 * the one thing worth seeing at a glance is which rows are work that
 * disappears if forced (destructive) and which are merely in the way
 * (muted) — the row's own text says what it is.
 *
 * An unrecognized kind (an older frontend meeting a newer backend across the
 * IPC boundary) gets a neutral alert glyph. The row still reads fine — its
 * message and remedy come from Rust — and admitting we don't know beats
 * asserting the wrong thing. */
function BlockerIcon({ kind, losesWork }: { kind: string; losesWork: boolean }) {
  const Icon = BLOCKER_ICONS[kind as SlotBlockerKind] ?? CircleAlert;
  return (
    <Icon
      className={cn(
        "mt-0.5 size-4 shrink-0",
        losesWork ? "text-destructive" : "text-muted-foreground",
      )}
      aria-hidden
    />
  );
}

export function AgentboardScreen() {
  const state = useAgentboardState();
  const { snapshot } = useStoreSnapshot();
  const { openTab, activeTab, openSettingsTab } = useWorkspace();
  // Deep-link focus: a "needs you" popover row scrolls its repo into view here.
  const focusRef = useFocusTarget<HTMLDivElement>("agentboard");
  const now = useNow(30_000);

  // One-shot "prompt cache about to expire" toast per session per cache
  // generation. `cacheExpiresAt` moves forward on every request Claude makes,
  // so keying on `sessionId:cacheExpiresAt` naturally re-arms the toast after
  // the session is nudged — while the 30s `useNow` tick can't re-fire the same
  // warning. The set is tiny (one entry per warning ever shown this mount),
  // so it's never pruned.
  const cacheWarned = useRef(new Set<string>());
  useEffect(() => {
    for (const repo of state.repos)
      for (const folder of repo.folders)
        for (const s of folder.sessions) {
          const d = s.agentState?.details;
          if (!s.live || !isAgent(s) || !d?.cacheExpiresAt) continue;
          if (!isCacheExpiring(d, now)) continue;
          const key = `${s.id}:${d.cacheExpiresAt}`;
          if (cacheWarned.current.has(key)) continue;
          cacheWarned.current.add(key);
          toast(
            `◔ ${folder.name} / ${s.name} — prompt cache expires in ~${fmtMins(d.cacheExpiresAt - now)}. Any message re-warms it; a cold resume re-reads everything at full price.`,
          );
        }
  }, [state.repos, now]);

  const [selected, setSelected] = useState<Selected>(null);
  // Which pane tile (session, diff, files, or tombstone) last claimed the
  // click — the sole driver of the violet focus ring below. Deliberately
  // separate from `selected`: `selected` targets the session the toolbar's
  // Close/⌘D/⌘W and cache-badge actions act on, while this is purely "which
  // tile is visually active" and every pane kind can claim it, not just
  // sessions.
  const [focusedPaneId, setFocusedPaneId] = useState<string | null>(null);
  // The folder whose windows the main area shows — set by clicking a folder
  // header or a session row. Null until the user picks a folder.
  const [activeFolderDir, setActiveFolderDir] = useState<string | null>(null);
  // Track-repo dialog: strictly-manual path entry (no discovery, no scanning —
  // a standing product rule). Just an absolute path typed in, added via the
  // same `ab_add_repo` command every other add path uses.
  // ab-split-session picker: only shown when the active folder has more than
  // one session not already in the active window (a single candidate is
  // added directly — see `splitIntoWindow`).
  const [splitOpen, setSplitOpen] = useState(false);
  // Pending remove awaiting confirmation because it would kill live sessions.
  const [confirmRemove, setConfirmRemove] = useState<RemoveTarget | null>(null);
  // Pending worktree deletion — always confirmed (it deletes from disk).
  const [confirmDeleteWt, setConfirmDeleteWt] = useState<RemoveTarget | null>(null);
  // A delete the guards refused, with the reasons — see `performDeleteWorktree`
  // and the blocked-delete dialog. Holds the original target so each remedy
  // can retry the same removal without the user re-finding the row.
  const [blockedDelete, setBlockedDelete] = useState<BlockedDelete | null>(null);
  // The port whose "Stop it" is in flight — held until the follow-up removal
  // finishes too, so the whole dialog is inert for the duration. A single
  // value, not a set: `deleteBusy` disables every stop button the moment one
  // is running, so a second stop can never start alongside it.
  const [stoppingPort, setStoppingPort] = useState<number | null>(null);
  // Generation counter per worktree dir for the delete flow. Bumped when a
  // dir's flow starts and whenever one ends (cancel, force, success), so an
  // attempt that resolves after the user moved on can tell it's stale — a
  // `slot_stop_port` plus retry runs for seconds, and without this a removal
  // returning "blocked" would pop the dialog back open after it was
  // dismissed. Scoped per dir rather than one global counter so starting a
  // delete on a second worktree can't silently swallow the first one's
  // still-in-flight outcome.
  const deleteFlows = useRef(new Map<string, number>());
  const deleteFlowOf = (dir: string) => deleteFlows.current.get(dir) ?? 0;
  const bumpDeleteFlow = (dir: string) => deleteFlows.current.set(dir, deleteFlowOf(dir) + 1);
  // Folder dirs whose worktree is mid-delete (`slot_remove` in flight) — the
  // rail dims/disables that row for the duration, see `performDeleteWorktree`.
  const [deletingDirs, setDeletingDirs] = useState<Set<string>>(new Set());
  // Repo management lives on one surface (Settings → Agentboard → Repos); the
  // rail just links to it.
  const openRepoManager = () => {
    uiAction("repo.manage_opened", "agentboard");
    openSettingsTab({ tab: "agentboard" });
  };
  // Session awaiting the "what are you working toward?" prompt before Claude
  // actually launches — see `commitStartClaude`.
  const [startClaudeTarget, setStartClaudeTarget] = useState<StartClaudeTarget | null>(null);
  // Repo keys whose inline new-slot form is open — see InlineNewSlot. A form
  // stays embedded in the rail rather than a modal, so several repos can have
  // one open (or a create in flight) at once without blocking each other.
  const [openSlotForms, setOpenSlotForms] = useState<Set<string>>(new Set());
  // `slot_create` calls fired from an inline form and still running (or
  // failed) — rendered as a PendingSlotRow until they resolve. See
  // `createSlot`.
  const [pendingSlots, setPendingSlots] = useState<PendingSlot[]>([]);
  const [startClaudePrompt, setStartClaudePrompt] = useState("");
  // Session ids whose PTY is mounted (kept alive for scrollback), + their cwd.
  const [open, setOpen] = useState<string[]>([]);
  const cwds = useRef<Record<string, string>>({});
  // How a *crashed* session's shell died ("exited · Killed"), by session id.
  // Only crashes land here — a clean logout takes its pane with it (see
  // `handleExit`). Entries are never invalidated: what's on screen is decided
  // by the render filter (a tombstone needs a pane that still exists and no
  // live terminal over the top), so a stale entry for a dismissed or reopened
  // session is inert, and there's no invalidation scheme to keep correct.
  const [exitLabels, setExitLabels] = useState<Record<string, string>>({});
  // Sessions whose shell we're killing on purpose. `slot_remove` kills a
  // folder's PTYs in Rust *before* the frontend unmounts their panes, so those
  // deaths arrive as signal exits at a still-listening TerminalView — which is
  // a crash by every test `handleExit` can apply, except that we asked for it.
  // Ids land here just before the kill and are consumed by the exit they
  // predict. (The `term_kill` on TerminalView unmount needs no entry: cleanup
  // unlistens first, so that exit is never delivered.)
  const expectedKills = useRef<Set<string>>(new Set());
  // Folder-rail collapse/expand state (issue #52): hydrated once from
  // `ab_get_state`, then this local copy is the live truth — same pattern as
  // `wins` below, except each toggle saves incrementally (one key at a time)
  // rather than a debounced whole-blob save, since a collapse entry is never
  // ambiguous between "not yet toggled" and "explicitly reset".
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const hydratedCollapsed = useRef(false);
  useEffect(() => {
    if (!hydratedCollapsed.current && state.ts > 0) {
      hydratedCollapsed.current = true;
      setCollapsed(state.collapsed);
    }
  }, [state.ts, state.collapsed]);

  function toggleCollapsed(key: string) {
    setCollapsed((c) => {
      const next = !c[key];
      void invoke("ab_save_collapsed", { key, collapsed: next });
      return { ...c, [key]: next };
    });
  }

  // Whole-rail icon collapse (issue #70): same persisted map, sentinel key.
  const railCollapsed = !!collapsed[RAIL_COLLAPSE_KEY];
  const toggleRail = () => toggleCollapsed(RAIL_COLLAPSE_KEY);

  // "Hide inactive" rail filter: demote quiet folders (see `isFolderQuiet` —
  // no live session, no dirty tree/unpushed commits, no session that catches
  // the eye, no agent activity within the grace window) behind a per-repo
  // "N quiet" stub row, so a big rail shrinks to what's actually going on
  // without anything silently disappearing. A view filter, not a
  // rail-structure change — local state only, unlike `collapsed` it doesn't
  // need to survive a reload. Lookups used for panes/sessions (folderOf,
  // sessionById, etc. below) stay on the full `repos` list; only the two
  // render surfaces (RepoGroup list, RailIconStrip) apply the filter, since
  // a pane already open for a now-quiet folder must keep working.
  const [hideInactive, setHideInactive] = useState(false);
  // Per-repo "show me the quiet ones anyway" toggle (the stub row).
  const [quietRevealed, setQuietRevealed] = useState<Record<string, boolean>>({});

  const [renaming, setRenaming] = useState<string | null>(null);
  const [renamingWin, setRenamingWin] = useState<string | null>(null);
  // Live PTY window titles keyed by session id (Claude emits `✳ <title>`);
  // preferred over the backend label for sessions whose terminal is open.
  const [titles, setTitles] = useState<Record<string, string>>({});
  const onTitle = (id: string, title: string) =>
    setTitles((m) => (m[id] === title ? m : { ...m, [id]: title }));
  // Sessions whose program raised attention (BEL / OSC 9 notification —
  // Claude Code asking for input) since the user last looked at them.
  // Set by the terminal://notify listener below, cleared on select.
  const [termAttention, setTermAttention] = useState<Record<string, true>>({});
  // Read live by the listener without re-subscribing on selection changes.
  const selectedRef = useRef<string | null>(null);
  useEffect(() => {
    selectedRef.current = selected?.sessionId ?? null;
  });
  // The label to lead a session row/tab with: the live Claude terminal title
  // when the shell is actually running, else the backend-derived task/shell
  // name. Gating on `s.live` keeps a stopped shell from showing the `✳ <goal>`
  // title its dead PTY last emitted (the `titles` map is never cleared), which
  // otherwise reads as a running Claude while the status says "not started".
  const labelFor = (s: SessionData) =>
    (s.live ? claudeTitleName(titles[s.id]) : null) ?? sessionLabel(s);

  const repos = state.repos;

  // Quiet checkout dirs per repo key, when the "hide inactive" filter is on.
  // The active folder never counts as quiet, so switching away from what
  // you're looking at never happens as a side effect of the filter. `now`
  // ticks every 30s, which is plenty for the 45-minute quiet grace window.
  const quietDirs = useMemo(() => {
    const m = new Map<string, Set<string>>();
    if (!hideInactive) return m;
    for (const r of repos) {
      const q = new Set(
        r.folders
          .filter((f) => isFolderQuiet(f, now) && f.dir !== activeFolderDir)
          .map((f) => f.dir),
      );
      if (q.size > 0) m.set(r.key, q);
    }
    return m;
  }, [repos, hideInactive, activeFolderDir, now]);

  // The collapsed icon strip has no room for stub rows, so there the filter
  // still just drops quiet (un-revealed) folders and any repo left empty.
  const visibleRepos = useMemo(() => {
    if (!hideInactive) return repos;
    return repos
      .map((r) => {
        const q = quietDirs.get(r.key);
        if (!q || quietRevealed[r.key]) return r;
        return { ...r, folders: r.folders.filter((f) => !q.has(f.dir)) };
      })
      .filter((r) => r.folders.length > 0);
  }, [repos, hideInactive, quietDirs, quietRevealed]);

  // Ghost checkouts (dir gone from disk) drive the one-click cleanup button.
  const missingRepoCount = useMemo(
    () => repos.flatMap((r) => r.folders).filter((f) => f.dirMissing).length,
    [repos],
  );

  // Index every session by id → its folder / its data, for cwd + pane chrome.
  const folderOf = useMemo(() => {
    const m = new Map<string, FolderData>();
    for (const r of repos) for (const f of r.folders) for (const s of f.sessions) m.set(s.id, f);
    return m;
  }, [repos]);
  // Folder dir → the backend's tracker name for it, so selecting into a
  // folder can ack its `unseen` agents (`ab_mark_seen`).
  const folderNameByDir = useMemo(() => {
    const m = new Map<string, string>();
    for (const r of repos) for (const f of r.folders) m.set(f.dir, f.name);
    return m;
  }, [repos]);
  const sessionById = useMemo(() => {
    const m = new Map<string, SessionData>();
    for (const r of repos) for (const f of r.folders) for (const s of f.sessions) m.set(s.id, s);
    return m;
  }, [repos]);
  // Folder dir → its owning repo, so a pane header can lead with "repo /
  // folder" (a folder's own name is just the checkout/slot/worktree).
  const repoOf = useMemo(() => {
    const m = new Map<string, RepoData>();
    for (const r of repos) for (const f of r.folders) m.set(f.dir, r);
    return m;
  }, [repos]);

  // The active folder resolved to its data + repo — drives the
  // working-context band ("where am I working, and why").
  const activeFolder = useMemo(
    () => repos.flatMap((r) => r.folders).find((f) => f.dir === activeFolderDir),
    [repos, activeFolderDir],
  );
  const activeRepo = activeFolder ? repoOf.get(activeFolder.dir) : undefined;

  // Folder dir → its data, for the diff panes (their pane id carries the dir).
  const folderByDir = useMemo(() => {
    const m = new Map<string, FolderData>();
    for (const r of repos) for (const f of r.folders) m.set(f.dir, f);
    return m;
  }, [repos]);

  // Open a folder's diff as a pane in its focused window (beside the live
  // terminals — never a modal). Re-opening focuses the window it's already in.
  function openDiff(dir: string) {
    setActiveFolderDir(dir);
    addPaneToActive(dir, diffPaneId(dir));
  }

  // Same, for the folder's full file tree.
  function openFiles(dir: string) {
    setActiveFolderDir(dir);
    addPaneToActive(dir, filesPaneId(dir));
  }

  // Same, for the folder's live dev-server preview (embedded browser + draw-on-
  // page feedback to this task's own session).
  function openPreview(dir: string) {
    setActiveFolderDir(dir);
    addPaneToActive(dir, previewPaneId(dir));
  }

  // Claude called the openFile tool → open (or focus) that folder's files
  // pane and focus the file. Routed here rather than inside the pane so the
  // request can *create* the pane when none is open yet. Latest-callback ref:
  // the listener registers once, the handler sees fresh state.
  const [filesOpenRequests, setFilesOpenRequests] = useState<Record<string, FilesOpenRequest>>({});
  const onOpenFileRequest = useRef<(p: OpenFileRequest) => void>(() => {});
  onOpenFileRequest.current = (p) => {
    const dir = p.dir;
    if (!folderByDir.has(dir)) return;
    const path = p.filePath.startsWith(`${dir}/`) ? p.filePath.slice(dir.length + 1) : p.filePath;
    setFilesOpenRequests((prev) => ({
      ...prev,
      [dir]: {
        path,
        anchor: {
          startText: p.startText,
          endText: p.endText,
          selectToEndOfLine: p.selectToEndOfLine,
        },
        nonce: nextOpenFileNonce(),
      },
    }));
    openFiles(dir);
  };
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const sub = await listen<OpenFileRequest>("ide://open-file", (e) =>
        onOpenFileRequest.current(e.payload),
      );
      if (disposed) sub();
      else unlisten = sub;
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Attention signals from terminals: a BEL or a desktop notification
  // (OSC 9/777 — Claude Code's "needs your input"). The session badges
  // amber until selected; a notification body also toasts, since the pane
  // raising it is usually not the one on screen.
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const sub = await listen<{ termId: string; kind: string; body?: string }>(
        "terminal://notify",
        (e) => {
          const { termId, kind, body } = e.payload;
          // The session the user is looking at doesn't need a badge.
          if (termId === selectedRef.current && document.hasFocus()) return;
          setTermAttention((m) => (m[termId] ? m : { ...m, [termId]: true }));
          if (kind === "notify" && body) toast(body);
        },
      );
      if (disposed) sub();
      else unlisten = sub;
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // --- Window layout (Tier 5): frontend-owned, hydrated once, saved debounced.
  const [wins, setWins] = useState<WindowsPayload | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Folder dirs actually mutated since the last flush — the backend merges
  // by folder dir on save, so it needs to know which ones we touched (a
  // never-hydrated-vs-explicitly-emptied folder look identical in the blob
  // alone; see `WindowsStore::save`'s doc comment).
  const dirtyWinFolders = useRef<Set<string>>(new Set());
  useEffect(() => {
    // Hydrate from the first real payload (mock or ab_get_state); after that
    // the local copy is the live truth and only flows outward. `hydrateWins`
    // is the parse boundary: paneless windows restored from old blobs are
    // residue (the empty-pane state is unrepresentable now) — swept there,
    // and the sweep is persisted if it changed anything.
    if (wins !== null || state.ts === 0) return;
    const hydrated = hydrateWins(state.windows);
    setWins(hydrated);
    const touched = changedFolderDirs(state.windows, hydrated);
    if (touched.length > 0) scheduleSave(hydrated, touched);
  }, [wins, state.ts, state.windows]);

  function scheduleSave(next: WindowsPayload, folderDirs: string[]) {
    for (const dir of folderDirs) dirtyWinFolders.current.add(dir);
    if (saveTimer.current) clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      const touchedFolders = [...dirtyWinFolders.current];
      dirtyWinFolders.current = new Set();
      void invoke("ab_save_windows", { payload: next, touchedFolders });
    }, 400);
  }

  function updateWins(folderDirs: string[], fn: (w: WindowsPayload) => WindowsPayload) {
    setWins((prev) => {
      const next = normalizeWins(fn(prev ?? { windows: [], activeWindows: {} }));
      scheduleSave(next, folderDirs);
      return next;
    });
  }

  // Reconcile the layout against reality whenever either changes: sessions
  // and folders vanish out from under the persisted blob (closed by another
  // slot's app instance, a repo removed with non-live session records, a
  // crash before the debounced save), leaving ghost pane ids that hold a tile
  // slot with nothing to render in it. Locally-mounted terminals (`open`)
  // count as valid even before the backend's state event catches up, so a
  // just-created session's pane never loses the race to this prune — and so
  // do their folders (via the cwd recorded at mount): a just-created slot's
  // window is keyed on a folder dir the backend hasn't broadcast yet, and
  // without that carve-out this prune ate the whole window (and persisted the
  // loss), leaving the new slot's main area empty until re-clicked.
  useEffect(() => {
    if (!wins) return;
    const validSessions = new Set(open);
    const validFolders = new Set<string>();
    for (const id of open) {
      const dir = cwds.current[id];
      if (dir) validFolders.add(dir);
    }
    for (const r of repos)
      for (const f of r.folders) {
        validFolders.add(f.dir);
        for (const s of f.sessions) validSessions.add(s.id);
      }
    const next = pruneWins(wins, validSessions, validFolders);
    if (next !== wins) {
      updateWins(changedFolderDirs(wins, next), (cur) =>
        pruneWins(cur, validSessions, validFolders),
      );
    }
    // updateWins is stable within a render pass; wins/repos/open are the inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wins, repos, open]);

  // Windows belonging to the active folder, and whichever of those is focused.
  const windowsForFolder = useMemo(
    () => wins?.windows.filter((w) => w.folderDir === activeFolderDir) ?? [],
    [wins, activeFolderDir],
  );
  const activeWin =
    windowsForFolder.find(
      (w) => w.id === (activeFolderDir && wins?.activeWindows[activeFolderDir]),
    ) ?? windowsForFolder[0];

  // The active folder's sessions not currently a pane in *any* of its
  // windows — what ab-split-session (⌘⇧S) has to choose from. Deliberately
  // folder-wide, not just the active window: `selectSession` (via
  // `placePane`) never moves a pane that already has a window, it just
  // switches focus to wherever it lives — so a session parked in another
  // window isn't a real candidate, it'd just yank focus away from the
  // window you're trying to add *to*.
  const splitCandidates = useMemo(() => {
    if (!activeFolder) return [];
    const openIds = new Set(windowsForFolder.flatMap((w) => w.panes));
    return activeFolder.sessions.filter((s) => !openIds.has(s.id));
  }, [activeFolder, windowsForFolder]);

  // ab-split-session: add one of the active folder's not-yet-opened sessions
  // as a pane in its active window. One candidate adds directly (mirrors
  // clicking it); more than one opens a picker, since a single keypress
  // can't disambiguate.
  function splitIntoWindow() {
    if (!activeFolderDir) {
      toast("Select a folder first.");
      return;
    }
    if (splitCandidates.length === 0) {
      toast("No unopened sessions in this folder to add.");
      return;
    }
    if (splitCandidates.length === 1) {
      selectSession(activeFolderDir, splitCandidates[0].id);
      return;
    }
    setSplitOpen(true);
  }

  // Add a pane (session or diff) to its own folder's focused window — the
  // placement rules live in the pure `placePane` reducer (lib/agentboard.ts).
  // A session reclaims its own tombstone first: the crashed pane is that
  // session's slot, so reopening fills it in place instead of `placePane`
  // appending a second pane beside the corpse.
  function addPaneToActive(folderDir: string, paneId: string) {
    updateWins([folderDir], (w) =>
      placePane(replacePane(w, exitPaneId(paneId), paneId), folderDir, paneId, nextWindowId),
    );
  }

  function removePane(paneId: string) {
    // A pane lives in exactly one folder's window; find it before mutating
    // so we know which single folder to mark touched.
    const folderDir = wins?.windows.find((win) => win.panes.includes(paneId))?.folderDir;
    updateWins(folderDir ? [folderDir] : [], (w) => dropPane(w, paneId));
  }

  /** Remove whichever pane a session currently occupies — its terminal, or the
   * tombstone that replaced it when the shell crashed. Every session-keyed
   * entry point (rail ungroup, pane header, close, worktree delete) goes
   * through here, so none of them has to know which of the two it's looking
   * at. */
  function removeSessionPane(sessionId: string) {
    const ids = [sessionId, exitPaneId(sessionId)];
    const folderDir = wins?.windows.find((win) =>
      ids.some((id) => win.panes.includes(id)),
    )?.folderDir;
    updateWins(folderDir ? [folderDir] : [], (w) => ids.reduce((acc, id) => dropPane(acc, id), w));
  }

  // "+ window": a window can't exist without panes, so minting one means
  // giving it content — spawn a fresh session and open the new window around
  // it in one move.
  async function newWindow(folderDir: string) {
    const added = await invoke<SessionData>("ab_add_session", { dir: folderDir, name: null });
    if (added.isErr()) return;
    const sessionId = added.value.id;
    const id = nextWindowId();
    updateWins([folderDir], (cur) => {
      const count = cur.windows.filter((w) => w.folderDir === folderDir).length;
      return {
        windows: [
          ...cur.windows,
          { id, name: `window ${count + 1}`, folderDir, panes: [sessionId] },
        ],
        activeWindows: { ...cur.activeWindows, [folderDir]: id },
      };
    });
    // Mount + focus the session; `placePane` sees it already hosted here.
    selectSession(folderDir, sessionId);
  }

  // --- Column resize: drag the divider between two side-by-side panes. Live
  // widths ride local state so the terminals reflow while dragging; the
  // result commits to the window's `cols` (debounced save) on release.
  // `dragCol` snaps to thirds/fifths of the tiling width.
  const paneAreaRef = useRef<HTMLDivElement>(null);
  const [colDrag, setColDrag] = useState<{ winId: string; cols: number[] } | null>(null);

  function startColDrag(e: ReactPointerEvent<HTMLDivElement>, win: AgWindow, divider: number) {
    e.preventDefault();
    const area = paneAreaRef.current;
    if (!area) return;
    const n = win.panes.length;
    const posOf = (clientX: number) => {
      const r = area.getBoundingClientRect();
      return ((clientX - r.left) / r.width) * COL_TOTAL;
    };
    let cols = dragCol(n, win.cols, divider, posOf(e.clientX));
    const move = (ev: PointerEvent) => {
      cols = dragCol(n, win.cols, divider, posOf(ev.clientX));
      setColDrag({ winId: win.id, cols });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setColDrag(null);
      updateWins([win.folderDir], (w) => ({
        ...w,
        windows: w.windows.map((x) => (x.id === win.id ? { ...x, cols } : x)),
      }));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    setColDrag({ winId: win.id, cols });
  }

  /** Double-click a divider: back to equal columns. */
  function resetCols(win: AgWindow) {
    updateWins([win.folderDir], (w) => ({
      ...w,
      windows: w.windows.map((x) => (x.id === win.id ? { ...x, cols: undefined } : x)),
    }));
  }

  /** A shell exited on its own. Either way its terminal unmounts (the PTY is
   * gone); how it died decides whether the pane goes with it.
   *
   * A clean logout is expected — you typed `exit`, and the pane disappearing
   * *is* the feedback; the window retiles around the loss. A crash is the
   * opposite: nothing would otherwise tell you it happened, so the pane stays
   * as a tombstone reporting how it died, until you dismiss it or reopen the
   * session over the top. A toast fires alongside, since the pane only speaks
   * to whoever is looking at that folder's window. No auto-restart. */
  function handleExit(sessionId: string, exit: TermExit) {
    setOpen((prev) => prev.filter((id) => id !== sessionId));
    const expected = expectedKills.current.delete(sessionId);
    if (expected || !exitIsCrash(exit.code, exit.signal)) {
      removePane(sessionId);
      return;
    }
    const label = exitLabel(exit.code, exit.signal);
    const s = sessionById.get(sessionId);
    toast.error(`${s ? labelFor(s) : "shell"} ${label}`);
    setExitLabels((m) => ({ ...m, [sessionId]: label }));
    // The slot keeps its place in the tiling; only its occupant changes.
    const folderDir = wins?.windows.find((win) => win.panes.includes(sessionId))?.folderDir;
    updateWins(folderDir ? [folderDir] : [], (w) =>
      replacePane(w, sessionId, exitPaneId(sessionId)),
    );
  }

  // Switch the main area to a folder without selecting one of its sessions
  // (clicking a folder header). Drops any selection from a *different*
  // folder so the cache bar / ⌘D / ⌘W / Close button never act on a session
  // that's no longer the one shown — a session selected in the folder you're
  // switching to stays selected.
  function selectFolder(folderDir: string) {
    setActiveFolderDir(folderDir);
    setSelected((cur) => (cur && cur.folderDir !== folderDir ? null : cur));
    ackFolder(folderDir);
  }

  // Spawn a session's PTY and place its pane in its own folder's window,
  // without touching `selected`/`activeFolderDir` — for sessions created in
  // the background (e.g. a new slot) that shouldn't steal focus from
  // whatever the user is currently looking at.
  function mountSession(folderDir: string, sessionId: string) {
    cwds.current[sessionId] = folderDir;
    setOpen((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
    addPaneToActive(folderDir, sessionId);
  }

  function selectSession(folderDir: string, sessionId: string) {
    mountSession(folderDir, sessionId);
    setSelected({ folderDir, sessionId });
    setFocusedPaneId(sessionId);
    setActiveFolderDir(folderDir);
    // Looking at it acknowledges it — drop the attention badge.
    setTermAttention((m) => {
      if (!m[sessionId]) return m;
      const { [sessionId]: _, ...rest } = m;
      return rest;
    });
    ackFolder(folderDir);
  }

  /**
   * Run `fn` against a session's PTY, guaranteeing its shell exists first.
   *
   * A pane spawns its shell only once rendered, and only the active folder's
   * active window renders — so "write to session X" really means "make X
   * visible, wait for its shell, then write". Every PTY-writing path goes
   * through here: open-coding the three steps is how stop/compact came to
   * silently no-op for any folder that wasn't the active one.
   *
   * `folderDir` is only needed when the session isn't on the board yet (the
   * crash-resume handoff at boot); otherwise it's resolved from state, so
   * callers don't have to carry it.
   */
  async function withLiveSession(
    sessionId: string,
    fn: () => Promise<unknown>,
    folderDir?: string,
  ) {
    const dir = folderDir ?? folderOf.get(sessionId)?.dir ?? cwds.current[sessionId];
    if (!dir) return;
    selectSession(dir, sessionId);
    await waitForFirstFrame(sessionId);
    await fn();
  }

  // The user is now looking at this folder's rail entry — clear its agents'
  // `unseen` flags (`sessionCatchesEye`'s pulse) via the backend tracker.
  function ackFolder(folderDir: string) {
    const name = folderNameByDir.get(folderDir);
    if (name) void invoke("ab_mark_seen", { name });
  }

  // ab-jump-next/ab-jump-prev (see lib/shortcuts.tsx): board-wide, wraps
  // around, reuses `selectSession` — the same "mount + focus + ack" path a
  // rail click uses — so a jump behaves exactly like clicking the session.
  function jumpToNeedsYou(direction: "next" | "prev") {
    const target = cycleNeedsYou(repos, selected?.sessionId ?? null, direction);
    if (!target) {
      toast("Nothing needs you right now.");
      return;
    }
    const folderDir = folderOf.get(target.id)?.dir;
    if (!folderDir) return;
    selectSession(folderDir, target.id);
  }

  // Toggle the inline new-slot form open/closed for a repo — the "+"/"New
  // slot…" affordances all funnel through this, same as clicking it again
  // closes the form rather than only ever opening one.
  function toggleSlotForm(repo: NewSlotRepo) {
    setOpenSlotForms((prev) => {
      const next = new Set(prev);
      if (next.has(repo.key)) next.delete(repo.key);
      else next.add(repo.key);
      return next;
    });
  }

  // ab-new-slot + the working-context band's "New slot" button both open the
  // form for the focused folder's repo — expand a collapsed rail first since
  // the form itself renders there, same as the rail's own new-slot buttons.
  function newSlotForActiveRepo() {
    if (!activeRepo) return;
    if (railCollapsed) toggleRail();
    toggleSlotForm({ name: activeRepo.name, dir: activeRepo.folders[0].dir, key: activeRepo.key });
  }

  function closeSlotForm(key: string) {
    setOpenSlotForms((prev) => {
      if (!prev.has(key)) return prev;
      const next = new Set(prev);
      next.delete(key);
      return next;
    });
  }

  // The setup step (npm install/etc.) can fail without invalidating the slot
  // itself — `slot_create`'s warning already says so. Give it a one-click
  // retry rather than making the user remember to re-run it from a terminal.
  async function retrySetup(dir: string) {
    (await invoke<string | null>("slot_run_setup", { dir })).match({
      ok: (warning) => {
        if (warning) toast(warning, { action: retryAction(dir) });
        else toast("setup succeeded");
      },
      err: (e) => toast(e.message),
    });
  }

  function retryAction(dir: string) {
    return { label: "Retry", onClick: () => void retrySetup(dir) };
  }

  // Fires `slot_create` in the background and tracks it as a PendingSlotRow
  // in the rail instead of a blocking modal — the caller can keep working
  // anywhere else in the app while the worktree + setup finish. Keyed by
  // branch (unique per repo, since a collision is already rejected before
  // submit), so a retry just re-runs this under the same id. The board task
  // is created first (`createTaskForSubmit`) — the slot is an attribute of
  // the task, not the unit itself — and bound to the worktree once
  // `slot_create` resolves; a "task only" submit stops after the card.
  async function createSlot(repo: NewSlotRepo, input: NewTaskSubmit & { taskId?: number }) {
    const taskId = input.taskId ?? (await createTaskForSubmit(input));
    // Bind the repo before any worktree exists. The Board groups tasks into
    // repo swimlanes, and the repo is known here — at the `+` the user clicked
    // — so binding it now is what keeps every task out of the "No repo" lane,
    // including a "task only" submit that never gets a branch or dir.
    if (taskId !== undefined) {
      void storeTaskSetSlot(taskId, repo.dir, undefined, {
        repo: ownerRepoFromOrigin(repo.originUrl),
      });
    }
    if (!input.worktree) {
      toast("task added to the board");
      return;
    }
    const id = `${repo.key}::${input.branch}`;
    setPendingSlots((prev) => [
      ...prev.filter((p) => p.id !== id),
      {
        id,
        repoKey: repo.key,
        repoDir: repo.dir,
        repoName: repo.name,
        goal: input.goal,
        branch: input.branch,
        base: input.base,
        options: input.options,
        imagePaths: input.imagePaths,
        taskId,
        dynamic: input.dynamic,
        repoOriginUrl: repo.originUrl,
        startedAt: Date.now(),
        status: "creating",
      },
    ]);
    const imagePaths = input.imagePaths;
    const result = await invoke<SlotCreated>(
      "slot_create",
      { root: repo.dir, branch: input.branch, base: input.base },
      { schema: SlotCreatedSchema, timeoutMs: 12 * 60_000 },
    );
    if (result.isErr()) {
      const error = result.error.message;
      setPendingSlots((prev) =>
        prev.map((p) => (p.id === id ? { ...p, status: "error" as const, error } : p)),
      );
      return;
    }
    const created = result.value;
    // Bind the task to its worktree (branch + dir + repo identity for PR
    // auto-attach). Fire-and-forget: the snapshot re-emit repaints the card.
    if (taskId !== undefined) {
      void storeTaskSetSlot(taskId, repo.dir, created.branch, {
        repo: ownerRepoFromOrigin(repo.originUrl),
        dir: created.dir,
      });
    }
    for (const warning of created.warnings) {
      toast(
        warning,
        warning.startsWith("setup `") ? { action: retryAction(created.dir) } : undefined,
      );
    }
    setPendingSlots((prev) => prev.filter((p) => p.id !== id));

    // An image with no typed goal is still a valid ask — give the rail
    // something to show rather than an unlabeled session.
    const label =
      input.goal ||
      (imagePaths.length ? `attached ${imagePaths.length === 1 ? "image" : "images"}` : "");
    // A dynamic task wraps the goal with the post-plan-approval delivery
    // pipeline and launches in plan mode — the base comes from the resolved
    // create (what the branch actually forked from), not the form field.
    const goalPrompt = input.dynamic ? dynamicFlowPrompt(input.goal, created.base) : input.goal;
    const launchOptions: ClaudeLaunchOptions = input.dynamic
      ? { ...input.options, permissionMode: "plan" }
      : input.options;
    await slotCreated(created, promptWithImages(goalPrompt, imagePaths), launchOptions, label);
  }

  function retryPendingSlot(id: string) {
    const p = pendingSlots.find((x) => x.id === id);
    if (!p) return;
    void createSlot(
      { name: p.repoName, dir: p.repoDir, key: p.repoKey, originUrl: p.repoOriginUrl },
      {
        goal: p.goal,
        branch: p.branch,
        base: p.base,
        options: p.options,
        imagePaths: p.imagePaths,
        // The task already exists — a retry must rebind it, not mint a
        // duplicate card. (Issues are already attached to it, too.)
        issues: [],
        worktree: true,
        dynamic: p.dynamic,
        taskId: p.taskId,
      },
    );
  }

  function dismissPendingSlot(id: string) {
    setPendingSlots((prev) => prev.filter((p) => p.id !== id));
  }

  // A slot the inline form just created: track it in the rail, mount its
  // first session in the background, and start Claude on the goal in that
  // session's PTY — without switching the user's current view over to it.
  // They can jump to it via the rail whenever they're ready.
  async function slotCreated(
    created: SlotCreated,
    prompt: string,
    options: ClaudeLaunchOptions,
    /** The goal as the user typed it — what the rail and the toast show, so
     * the image paths `promptWithImages` appended stay out of both. */
    label?: string,
  ) {
    toast(`created ${created.name}${created.branch ? ` on ${created.branch}` : ""}`);
    await invoke("ab_add_repo", { path: created.dir });
    // A freshly tracked folder already gets a default not-started session —
    // reuse it rather than adding a second one, which would open as a
    // surprise split pane beside the empty default.
    const fresh = await invoke<StatePayload>("ab_get_state", {});
    const folder = fresh.isOk()
      ? fresh.value.repos.flatMap((r) => r.folders).find((f) => f.dir === created.dir)
      : undefined;
    let rec = folder?.sessions[0] ?? null;
    if (!rec) {
      const added = await invoke<SessionData>("ab_add_session", { dir: created.dir, name: null });
      if (added.isErr()) return;
      rec = added.value;
    }
    mountSession(created.dir, rec.id);
    if (prompt) {
      // `launchClaudeIn` waits for the PTY's first frame itself — a proxy for
      // "the shell is actually reading input", since a successful term_write
      // only proves the Rust-side conduit exists, not that zsh finished
      // sourcing its rc files.
      await launchClaudeIn(
        { folderDir: created.dir, sessionId: rec.id, sessionName: rec.name, restart: false },
        prompt,
        options,
        label,
      );
    }
  }

  async function newSession(folderDir: string, launchClaude = false) {
    const added = await invoke<SessionData>("ab_add_session", { dir: folderDir, name: null });
    if (added.isErr()) return;
    const rec = added.value;
    selectSession(folderDir, rec.id);
    if (launchClaude) {
      setStartClaudeTarget({ folderDir, sessionId: rec.id, sessionName: rec.name, restart: false });
    }
  }

  // Actually launch Claude in `target`'s session, folding in whatever prompt
  // the user entered (or none) — see `commitStartClaude`, which reads the
  // dialog state and calls this.
  async function launchClaudeIn(
    target: StartClaudeTarget,
    prompt: string,
    options?: ClaudeLaunchOptions,
    /** What the toast and the session's rail purpose show, when that should
     * differ from what's actually typed into the PTY — the new-slot flow
     * appends attached-image paths to `prompt` that would only be noise
     * here. Defaults to `prompt` for every other caller. */
    label?: string,
  ) {
    const { folderDir, sessionId, sessionName, restart } = target;
    const shown = label ?? prompt;
    setOverlay(sessionId, "busy");
    const verb = restart ? "starting over — fresh Claude session" : "starting Claude";
    toast(shown ? `✦ ${verb} in ${sessionName}: ${shown}` : `✦ ${verb} in ${sessionName}`);
    if (shown) void invoke("ab_set_session_purpose", { id: sessionId, text: shown });
    await withLiveSession(
      sessionId,
      async () => {
        if (restart) {
          await termWriteRetry(sessionId, "\x03");
          await sleep(150);
          await termWriteRetry(sessionId, "\x04");
          await sleep(300);
        }
        await termWriteRetry(sessionId, claudeCommand(prompt, options));
      },
      folderDir,
    );
  }

  // Start a `.claude/launch.json` dev-server config in a fresh session named
  // after it — the same PTY-typing path `launchClaudeIn` uses (no backend
  // spawn), then register the config→session mapping so the popover offers
  // "focus" instead of a second launch while the pane lives.
  async function launchDevServer(folderDir: string, cfg: LaunchConfigStatus) {
    const added = await invoke<SessionData>("ab_add_session", {
      dir: folderDir,
      name: `dev: ${cfg.name}`,
    });
    if (added.isErr()) {
      toast(errorMessage(added.error));
      return;
    }
    const rec = added.value;
    const command = launchCommand(cfg);
    toast(`▶ ${command} — in ${rec.name}`);
    void invoke("ab_set_session_purpose", { id: rec.id, text: command });
    await withLiveSession(
      rec.id,
      async () => {
        const wrote = await termWriteRetry(rec.id, `${command}\r`);
        if (wrote.isErr()) {
          toast(`could not start ${cfg.name}: ${errorMessage(wrote.error)}`);
          return;
        }
        void launchRegister(folderDir, cfg.name, rec.id, cfg.port ?? null, command);
      },
      folderDir,
    );
  }

  // Dismiss the start-Claude dialog (Enter, Escape, or click-outside all land
  // here via `onOpenChange`/`onKeyDown`) and launch with whatever's typed —
  // blank is a valid answer, it just skips the initial prompt + purpose.
  function commitStartClaude() {
    const target = startClaudeTarget;
    if (!target) return;
    setStartClaudeTarget(null);
    const prompt = startClaudePrompt.trim();
    setStartClaudePrompt("");
    void launchClaudeIn(target, prompt);
  }

  // Claude Sessions' "Open in Agentboard" handoff (see `lib/agentboard.ts`'s
  // pending-open-session bridge doc comment for why this can't be a plain
  // function call).
  //
  // Requests run **one at a time** via a promise tail: `withLiveSession` makes
  // each request's folder active to mount its pane, and only one folder can be
  // active at a time — so overlapping them would leave every folder but the
  // last with a pane that never started.
  useEffect(() => {
    let cancelled = false;
    let tail = Promise.resolve();

    const handle = (req: PendingOpenSession) => {
      tail = tail.then(async () => {
        if (cancelled) return;
        toast(`✦ resuming ${req.label} — claude --resume ${req.resumeId.slice(0, 8)}`);
        await withLiveSession(
          req.sessionId,
          () => termWriteRetry(req.sessionId, claudeResumeCommand(req.resumeId)),
          req.folderDir,
        );
      });
    };
    for (const req of consumePendingOpenSessions()) handle(req);
    const off = onOpenSessionRequest(handle);
    return () => {
      cancelled = true;
      off();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Command-palette "jump to repo/session" handoff (see `requestAgentboardNav`
  // in lib/agentboard.ts). Read-only reveal: focus the folder, and for a
  // session request select its pane too — no PTY writes, unlike the resume
  // handoff above.
  useEffect(() => {
    const handle = (req: AgentboardNav) => {
      if (req.kind === "session") {
        selectSession(req.folderDir, req.sessionId);
      } else {
        setActiveFolderDir(req.folderDir);
        ackFolder(req.folderDir);
      }
    };
    const pending = consumePendingAgentboardNav();
    if (pending) handle(pending);
    return onAgentboardNavRequest(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Sweep every "missing" ghost in one click. The Rust side re-probes the
  // disk at call time, so a directory restored since the last poll survives;
  // no sessions to kill — a missing dir has no live PTY.

  // Actually remove: kill any live sessions first (killing a PTY is
  // client-mediated — see `closeSession`/`TerminalView`'s unmount effect),
  // then drop the checkout(s) from the watched list. Removes by `dir`, never
  // by resolved session name — a multi-checkout repo removes several dirs in
  // one batch, and `ab_remove_repo`'s name resolution shifts as each removal
  // changes the collision-disambiguated names of whatever's left.
  async function performRemove(target: RemoveTarget) {
    // Closed here rather than by `untrackRepo` because `closeSession` also
    // clears this screen's local pane state (open list, selection, the pane
    // itself) — so the seam is handed an empty id list and owns only the
    // untrack, its `Result` check, and the `ui.action` event.
    for (const id of target.sessionIds) await closeSession(id);
    for (const dir of target.dirs) await untrackRepo(dir, target.label, [], "agentboard");
  }

  // Delete a worktree slot from disk. Always confirms (unlike untracking,
  // this touches the filesystem); the Rust side's guards still protect real
  // work — a dirty tree, commits unreachable from any branch/remote, or a
  // foreign listener on a claimed port come back as reasons instead of a
  // deletion (see the blocked-delete dialog, which offers each one's remedy).
  function requestDeleteWorktree(dir: string, label: string) {
    const folder = repos.flatMap((r) => r.folders).find((f) => f.dir === dir);
    const sessionIds = folder ? liveSessions(folder).map((s) => s.id) : [];
    bumpDeleteFlow(dir); // a fresh flow — see `endDeleteFlow`
    setConfirmDeleteWt({ label, dirs: [dir], sessionIds });
  }

  // Abandon the delete flow for `dir`: closes the blocked dialog and
  // invalidates any still-in-flight attempt, so a removal that resolves after
  // the user walked away can't reopen the dialog behind them. Every exit from
  // the blocked dialog goes through here.
  function endDeleteFlow(dir: string | undefined) {
    if (dir !== undefined) bumpDeleteFlow(dir);
    setBlockedDelete(null);
  }

  // `force` skips every guard — only ever passed from the blocked dialog's
  // force button, which names what's being discarded.
  async function performDeleteWorktree(target: RemoveTarget, { force = false } = {}) {
    // `slot_remove` kills the folder's live PTYs itself — only once the
    // guards have passed and the removal is really happening, so a refusal
    // costs nothing — and only tears down the session records once removal
    // actually succeeds; closing sessions here first would untrack them even
    // when removal is blocked (dirty tree, unpushed commits, a foreign
    // port), leaving the rail looking clean while the worktree stays on
    // disk. `deletingDirs` dims/disables the rail's row for this dir while
    // the (possibly slow — git checks, docker cleanup) call is in flight, so
    // it can't be clicked into or deleted twice; cleared at the end so a
    // blocked/failed removal leaves the row interactive again.
    const dir = target.dirs[0];
    const flow = deleteFlowOf(dir);
    setDeletingDirs((prev) => new Set(prev).add(dir));
    // Claim these deaths before asking for them — when removal proceeds, the
    // kill happens in Rust while the panes are still mounted, so the exits
    // come back as crashes. A blocked/failed attempt kills nothing, so the
    // unconsumed claims are handed back below — otherwise they'd linger and
    // silently swallow a later genuine crash of the same session.
    for (const id of target.sessionIds) expectedKills.current.add(id);
    const removed = await invoke<SlotRemoveOutcome>(
      "slot_remove",
      { dir, force },
      // Validated: a `foreignPort` blocker's `port` is handed straight to
      // `slot_stop_port`, which signals a process group.
      { schema: SlotRemoveOutcomeSchema },
    );
    // The user may have cancelled, or forced past this, while the call ran.
    // A stale result must not resurrect the dialog or re-report an outcome
    // for a flow that's over — but the `deletingDirs` release below still has
    // to run, or the rail row stays dimmed forever.
    const current = deleteFlowOf(dir) === flow;
    if (removed.isErr() || removed.value.status === "blocked") {
      // Nothing was removed, so no PTY was killed — return the claims.
      for (const id of target.sessionIds) expectedKills.current.delete(id);
    }
    removed.match({
      ok: (outcome) => {
        // Refused, not failed: hand the reasons to the dialog that can act on
        // them rather than a toast that can only be dismissed.
        if (outcome.status === "blocked") {
          if (current)
            setBlockedDelete({
              target,
              name: outcome.name,
              blockers: outcome.blockers,
              messages: outcome.messages,
            });
          return;
        }
        endDeleteFlow(dir);
        for (const id of target.sessionIds) {
          setOpen((prev) => prev.filter((x) => x !== id));
          setSelected((cur) => (cur?.sessionId === id ? null : cur));
          removeSessionPane(id);
        }
        for (const message of outcome.messages) toast(message);
        toast.success(`Deleted worktree ${outcome.name || target.label}`);
      },
      // A genuine failure (bad path, broken worktree, git fell over) — there
      // is no remedy to offer, so this stays a toast.
      err: (e) => {
        if (current) toast.error(e.message);
      },
    });
    setDeletingDirs((prev) => {
      const next = new Set(prev);
      next.delete(dir);
      return next;
    });
  }

  // Clear a stale dev server off one of the slot's claimed ports, then retry
  // the delete — the remedy for a `foreignPort` blocker, so the whole flow
  // finishes where it started instead of sending the user to a terminal.
  // `slot_stop_port` refuses any port the slot doesn't claim in its `.env`,
  // and only returns once the port is actually free, so the retry can't race
  // the socket's release.
  async function stopPortAndRetry(blocked: BlockedDelete, port: number) {
    const dir = blocked.target.dirs[0];
    // Captured before the stop runs (it takes seconds — SIGTERM, wait,
    // maybe SIGKILL): "Keep the worktree" stays clickable during it, and a
    // cancel bumps the flow, so this is what lets the check below actually
    // see the cancel. Capturing after the await would always read the
    // post-cancel value and retry anyway — deleting a worktree the user
    // just chose to keep.
    const flow = deleteFlowOf(dir);
    setStoppingPort(port);
    const stopped = await invoke<string>("slot_stop_port", { dir, port });
    if (stopped.isErr()) {
      toast.error(stopped.error.message);
    } else {
      // The stop really happened, so it's reported even if the user has
      // moved on — but the retry is theirs to want, not ours to assume.
      toast.success(stopped.value);
      // Re-run the guarded removal: the port is free now, but a dirty tree or
      // unreachable commits may still (correctly) block, in which case the
      // dialog just re-renders with one fewer reason. A port that was already
      // free comes back `Ok` too (the user may have quit the dev server
      // themselves after reading the blocker), so that also lands here rather
      // than dead-ending on an error toast.
      if (deleteFlowOf(dir) === flow) await performDeleteWorktree(blocked.target);
    }
    // Released only now, after the retry: clearing it before would re-enable
    // this row's button while the removal is still running, letting a second
    // row's "Stop it" start an overlapping removal of the same worktree.
    setStoppingPort(null);
  }

  // Any blocked-dialog action in flight — a port stop, or the removal that
  // follows it. Every button in that dialog ends in a removal of the same
  // worktree, so they share one gate rather than each disabling only itself.
  const blockedDeleteDir = blockedDelete?.target.dirs[0];
  // The removal itself (as opposed to the port stop before it) — once this
  // is true, "Keep the worktree" can no longer be honored, so the dialog's
  // cancel affordances lock too rather than promising an undo they can't do.
  const blockedRemovalInFlight = blockedDeleteDir != null && deletingDirs.has(blockedDeleteDir);
  const deleteBusy = stoppingPort !== null || blockedRemovalInFlight;

  // Remove a repo (or, for a multi-checkout repo, all its checkouts) from
  // the rail. Immediate when nothing's running; confirms first (see the
  // AlertDialog below) when any of its sessions are live, since confirming
  // kills them.
  function requestRemoveRepo(dirs: string[], label: string) {
    const folders = repos.flatMap((r) => r.folders).filter((f) => dirs.includes(f.dir));
    const sessionIds = folders.flatMap((f) => liveSessions(f).map((s) => s.id));
    const target: RemoveTarget = { label, dirs, sessionIds };
    if (sessionIds.length === 0) {
      void performRemove(target);
      return;
    }
    setConfirmRemove(target);
  }

  async function closeSession(sessionId: string) {
    await invoke("ab_close_session", { id: sessionId });
    setOpen((prev) => prev.filter((id) => id !== sessionId));
    setSelected((cur) => (cur?.sessionId === sessionId ? null : cur));
    removeSessionPane(sessionId);
  }

  async function commitRename(sessionId: string, name: string) {
    setRenaming(null);
    const trimmed = name.trim();
    if (trimmed) await invoke("ab_rename_session", { id: sessionId, name: trimmed });
  }

  // Optimistic lifecycle overlays (sessionId → forced status until ts). The
  // 2s watcher scan re-renders with ground truth; overlays just cover the gap.
  const [overlays, setOverlays] = useState<Record<string, Overlay>>({});
  const setOverlay = (id: string, status: AgentStatus) =>
    setOverlays((m) => ({ ...m, [id]: { status, until: Date.now() + 2_500 } }));

  const actions: SessionActions = {
    start: (folderDir, s) => {
      // Selecting mounts the TerminalView, whose effect spawns the PTY.
      selectSession(folderDir, s.id);
    },
    startClaude: (folderDir, s) => {
      selectSession(folderDir, s.id);
      setStartClaudeTarget({ folderDir, sessionId: s.id, sessionName: s.name, restart: false });
    },
    stopClaude: (s) => {
      setOverlay(s.id, "interrupted");
      toast(`■ interrupting Claude — ${s.name}'s shell stays alive`);
      void withLiveSession(s.id, async () => {
        await termWriteRetry(s.id, "\x03"); // interrupt the current turn
        await sleep(150);
        await termWriteRetry(s.id, "\x04"); // Ctrl-D at the empty prompt exits Claude
      });
    },
    compactClaude: (s) => {
      setOverlay(s.id, "busy");
      toast(`⤿ compacting ${s.name} — summarize & drop stale turns`);
      void withLiveSession(s.id, () => termWriteRetry(s.id, "/compact\r"));
    },
    restartClaude: (folderDir, s) => {
      selectSession(folderDir, s.id);
      setStartClaudeTarget({ folderDir, sessionId: s.id, sessionName: s.name, restart: true });
    },
    close: (sessionId) => void closeSession(sessionId),
    renameStart: setRenaming,
    ungroup: removeSessionPane,
    launchDevServer: (folderDir, cfg) => void launchDevServer(folderDir, cfg),
    focusSession: selectSession,
    focusWindow: (windowId) => {
      const win = wins?.windows.find((w) => w.id === windowId);
      if (!win) return;
      selectFolder(win.folderDir);
      updateWins([win.folderDir], (w) => ({
        ...w,
        activeWindows: { ...w.activeWindows, [win.folderDir]: windowId },
      }));
    },
  };

  // Agentboard-scoped shortcuts (see lib/shortcuts.tsx for the registry).
  // Gated on the tab being active: this screen stays mounted while hidden, so
  // without the gate ⌘D would spawn sessions from the Cockpit. Close-session
  // is ⌘⇧W (not ⌘W) — killing a shell deserves a deliberate chord.
  useShortcuts(
    useMemo(
      () => ({
        "ab-new-session": () => {
          if (activeFolderDir) void newSession(activeFolderDir);
        },
        "ab-new-slot": newSlotForActiveRepo,
        "ab-remove-slot": () => {
          // `requestDeleteWorktree` always confirms before touching anything;
          // the in-flight check mirrors the rail row dimming itself while a
          // removal runs.
          if (!activeFolder || !folderRemovableSlot(activeFolder)) return;
          if (deletingDirs.has(activeFolder.dir)) return;
          requestDeleteWorktree(activeFolder.dir, activeFolder.name);
        },
        "ab-close-session": () => {
          if (selected) void closeSession(selected.sessionId);
        },
        "ab-toggle-diff": () => {
          if (activeFolderDir) openDiff(activeFolderDir);
        },
        "ab-toggle-files": () => {
          if (activeFolderDir) openFiles(activeFolderDir);
        },
        "ab-toggle-preview": () => {
          if (activeFolderDir) openPreview(activeFolderDir);
        },
        "ab-toggle-rail": toggleRail,
        "ab-jump-next": () => jumpToNeedsYou("next"),
        "ab-jump-prev": () => jumpToNeedsYou("prev"),
        "ab-split-session": splitIntoWindow,
        "ab-new-terminal-right": () => {
          if (activeFolderDir) void newSession(activeFolderDir);
        },
      }),
      // newSession/closeSession/openDiff/openFiles/jumpToNeedsYou/splitIntoWindow are
      // stable within a render pass; the state they close over is what matters.
      // eslint-disable-next-line react-hooks/exhaustive-deps
      [
        activeFolderDir,
        deletingDirs,
        selected,
        wins,
        repos,
        folderOf,
        splitCandidates,
        activeRepo,
        railCollapsed,
      ],
    ),
    activeTab === "agentboard",
  );

  // Compact attention strip: failing/review PRs + the next imminent meeting.
  const attention = useMemo(() => {
    const items: {
      key: string;
      kind: "pr" | "event";
      title: string;
      sub: string;
      onClick: () => void;
    }[] = [];
    for (const p of snapshot.prs) {
      const checksFailing = p.state !== "merged" && p.checks === "failing";
      if (checksFailing || p.reviewState === "review_requested") {
        items.push({
          key: `pr:${p.repo}#${p.number}`,
          kind: "pr",
          title: `${p.repo.split("/").pop()} #${p.number}`,
          sub: checksFailing ? "Checks failing" : "Review requested",
          onClick: () => void openExternalUrl(p.url),
        });
      }
    }
    const soon = snapshot.events
      .filter((e) => e.startTs > now && e.startTs - now <= 30 * 60_000)
      .toSorted((a, b) => a.startTs - b.startTs)[0];
    if (soon) {
      items.push({
        key: `event:${soon.id}`,
        kind: "event",
        title: soon.title,
        sub: `Starts in ${fmtCountdown(soon.startTs - now)}`,
        onClick: () => openTab("cockpit"),
      });
    }
    return items;
  }, [snapshot.prs, snapshot.events, now, openTab]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1">
        {/* Rail collapsed to icons: fixed-width strip outside the panel group.
            The group itself is NOT keyed on the collapse — remounting it would
            remount the terminal pool below and respawn every shell. The rail
            panel + handle just unmount; the main panel keeps its identity. */}
        {railCollapsed && (
          <RailIconStrip
            repos={visibleRepos}
            activeFolderDir={activeFolderDir}
            attentionCount={attention.length}
            onSelectFolder={selectFolder}
            onExpand={toggleRail}
            expandHint={shortcutHint("ab-toggle-rail")}
          />
        )}
        <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
          {/* Rail: rollup tally + header + attention strip + Repo → Folder → Session tree. */}
          {!railCollapsed && (
            <>
              <ResizablePanel defaultSize="520px" minSize="220px" maxSize="760px">
                <div className="flex h-full flex-col border-r">
                  <RollupChip state={state} now={now} />
                  <div className="flex items-center justify-between border-b px-3 py-2">
                    <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Repos
                    </span>
                    <span className="flex items-center gap-0.5">
                      <button
                        type="button"
                        onClick={openRepoManager}
                        className="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs font-medium text-violet-500 hover:bg-accent/50"
                        title="Manage tracked repos in Settings — track, reorder, icon and color"
                      >
                        <FolderPlus className="size-3.5" /> Manage repos
                      </button>
                      {missingRepoCount > 0 && (
                        <button
                          type="button"
                          onClick={() => void cleanupMissing()}
                          aria-label={`Untrack ${missingRepoCount} missing repos`}
                          className="rounded-md p-1 text-amber-500 hover:bg-accent/50 hover:text-amber-400"
                          title={`Untrack ${missingRepoCount} repo${missingRepoCount === 1 ? "" : "s"} whose director${missingRepoCount === 1 ? "y is" : "ies are"} gone from disk`}
                        >
                          <FolderX className="size-3.5" />
                        </button>
                      )}
                      <button
                        type="button"
                        onClick={() => setHideInactive((v) => !v)}
                        aria-label={hideInactive ? "Show all repos" : "Hide inactive repos"}
                        aria-pressed={hideInactive}
                        className={cn(
                          "rounded-md p-1 hover:bg-accent/50",
                          hideInactive
                            ? "text-violet-500 hover:text-violet-400"
                            : "text-muted-foreground hover:text-foreground",
                        )}
                        title={
                          hideInactive
                            ? "Showing only repos with something going on — click to show all"
                            : "Hide repos with nothing going on (no live session, no dirty tree, no unpushed commits)"
                        }
                      >
                        {hideInactive ? (
                          <EyeOff className="size-3.5" />
                        ) : (
                          <Eye className="size-3.5" />
                        )}
                      </button>
                      <button
                        type="button"
                        onClick={toggleRail}
                        aria-label="Collapse the rail to icons"
                        className="rounded-md p-1 text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                        title={`Collapse the rail to icons (${shortcutHint("ab-toggle-rail")})`}
                      >
                        <PanelLeftClose className="size-3.5" />
                      </button>
                    </span>
                  </div>

                  {attention.length > 0 && (
                    <div className="flex flex-col gap-1 border-b p-2">
                      {attention.map((a) => (
                        <button
                          key={a.key}
                          type="button"
                          onClick={a.onClick}
                          className={cn(
                            "flex items-center gap-2 rounded-md border border-l-2 px-2 py-1.5 text-left hover:bg-accent/50",
                            a.kind === "pr" ? "border-l-red-500" : "border-l-blue-500",
                          )}
                        >
                          {a.kind === "pr" ? (
                            <GitPullRequest className="size-3.5 shrink-0 text-muted-foreground" />
                          ) : (
                            <CalendarClock className="size-3.5 shrink-0 text-muted-foreground" />
                          )}
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-xs font-medium">{a.title}</span>
                            <span className="block truncate text-[11px] text-muted-foreground">
                              {a.sub}
                            </span>
                          </span>
                        </button>
                      ))}
                    </div>
                  )}

                  {/* min-h-0 is load-bearing: without it this flex child grows past the
                      rail's height and folders below the fold become unreachable. */}
                  <ScrollArea className="min-h-0 flex-1">
                    <div ref={focusRef} className="flex flex-col">
                      {repos.length === 0 && (
                        <div className="flex flex-col items-center gap-3 px-3 py-10 text-center">
                          <FolderGit2 className="size-8 text-muted-foreground" />
                          <p className="text-sm text-muted-foreground">No repos on the rail yet.</p>
                          <div className="flex items-center gap-2">
                            <Button size="sm" variant="outline" onClick={openRepoManager}>
                              <FolderPlus className="size-3.5" /> Manage repos
                            </Button>
                          </div>
                        </div>
                      )}
                      {/* initial={false} so the rail drawing itself on launch
                          isn't mistaken for repos arriving — only genuine
                          track/untrack animates. */}
                      <AnimatePresence initial={false}>
                        {repos.map((repo) => (
                          <motion.div key={repo.key} {...railRowMotion}>
                            <RepoGroup
                              repo={repo}
                              quietDirs={quietDirs.get(repo.key)}
                              quietRevealed={!!quietRevealed[repo.key]}
                              onToggleQuiet={() =>
                                setQuietRevealed((m) => ({ ...m, [repo.key]: !m[repo.key] }))
                              }
                              now={now}
                              compactPct={state.compactRecommendPercent}
                              prs={snapshot.prs}
                              selectedSessionId={selected?.sessionId ?? null}
                              activeFolderDir={activeFolderDir}
                              collapsed={collapsed}
                              renaming={renaming}
                              titles={titles}
                              overlays={overlays}
                              wins={wins}
                              actions={actions}
                              onToggle={toggleCollapsed}
                              onSelectFolder={selectFolder}
                              onSelect={selectSession}
                              onNewSession={newSession}
                              onNewSlot={toggleSlotForm}
                              onRemoveRepo={requestRemoveRepo}
                              onDeleteWorktree={requestDeleteWorktree}
                              deletingDirs={deletingDirs}
                              onRenameCommit={commitRename}
                              onOpenDiff={openDiff}
                              onOpenFiles={openFiles}
                              onOpenPreview={openPreview}
                              slotFormOpen={openSlotForms.has(repo.key)}
                              onCancelSlotForm={() => closeSlotForm(repo.key)}
                              onSubmitSlotForm={(input) => {
                                closeSlotForm(repo.key);
                                void createSlot(
                                  {
                                    name: repo.name,
                                    dir: repo.folders[0].dir,
                                    key: repo.key,
                                    originUrl: repo.originUrl,
                                  },
                                  input,
                                );
                              }}
                              pendingSlots={pendingSlots.filter((p) => p.repoKey === repo.key)}
                              onRetryPendingSlot={retryPendingSlot}
                              onDismissPendingSlot={dismissPendingSlot}
                            />
                          </motion.div>
                        ))}
                      </AnimatePresence>
                    </div>
                  </ScrollArea>
                </div>
              </ResizablePanel>
              <ResizableHandle />
            </>
          )}

          {/* Main area: window strip + the active window's panes tiled side-by-side.
              Scoped to `activeFolderDir` — a window may only ever hold panes from
              the one folder it belongs to, so switching folders switches the
              whole strip, not just which panes happen to show. */}
          <ResizablePanel key="main">
            <div className="flex h-full min-w-0 flex-col">
              {activeFolder && activeRepo && (
                <WorkingContext
                  repo={activeRepo}
                  folder={activeFolder}
                  pr={prForFolder(snapshot.prs, activeRepo.originUrl, activeFolder.branch)}
                  actions={actions}
                  onOpenDiff={openDiff}
                  onOpenFiles={openFiles}
                  onOpenPreview={openPreview}
                  onNewSession={newSession}
                  onNewSlot={newSlotForActiveRepo}
                  onRemoveRepo={requestRemoveRepo}
                  onDeleteWorktree={requestDeleteWorktree}
                />
              )}
              {wins && activeFolderDir && (
                <div className="flex items-center gap-1 border-b bg-card px-2 py-1">
                  {windowsForFolder.map((w) =>
                    // Swap the chip for the input rather than nesting one
                    // inside it: buttons may not contain interactive
                    // descendants. See apps/client/CLAUDE.md.
                    renamingWin === w.id ? (
                      <input
                        key={w.id}
                        autoFocus
                        defaultValue={w.name}
                        aria-label={`rename window ${w.name}`}
                        onBlur={(e) => {
                          const name = e.target.value.trim() || w.name;
                          setRenamingWin(null);
                          updateWins([w.folderDir], (cur) => ({
                            ...cur,
                            windows: cur.windows.map((x) => (x.id === w.id ? { ...x, name } : x)),
                          }));
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                          if (e.key === "Escape") setRenamingWin(null);
                        }}
                        className="w-24 shrink-0 rounded-md border border-input bg-background px-2 py-1 text-[11px] outline-none"
                      />
                    ) : (
                      <button
                        key={w.id}
                        type="button"
                        onClick={() => actions.focusWindow(w.id)}
                        onDoubleClick={() => setRenamingWin(w.id)}
                        title="double-click to rename"
                        aria-pressed={w.id === activeWin?.id}
                        className={cn(
                          // border-b-2 mirrors the rail's border-l-2 active edge,
                          // rotated to match this strip's horizontal layout — kept
                          // transparent at rest so the violet edge never shifts
                          // the tab's size when it becomes active.
                          "flex shrink-0 items-center gap-1.5 rounded-md border-b-2 border-transparent px-2 py-1 text-[11px]",
                          w.id === activeWin?.id
                            ? "border-b-violet-500 bg-accent text-foreground"
                            : "text-muted-foreground hover:bg-accent/50",
                        )}
                      >
                        <span
                          className={cn(
                            "size-2 rounded-[3px]",
                            windowColor(windowsForFolder, w.id),
                          )}
                        />
                        {w.name}
                        <span className="font-mono text-[10px] text-muted-foreground/60">
                          {w.panes.length}⊞
                        </span>
                        {windowsForFolder.length > 1 && (
                          // span-with-role, not <button>: it nests inside the
                          // window chip's real <button>, and interactive elements
                          // may not nest. Keyboard support added by hand instead.
                          <span
                            role="button"
                            tabIndex={0}
                            title="close window (panes ungroup; sessions stay in the rail)"
                            aria-label={`close window ${w.name}`}
                            onClick={(e) => {
                              e.stopPropagation();
                              updateWins([w.folderDir], (cur) => ({
                                ...cur,
                                windows: cur.windows.filter((x) => x.id !== w.id),
                              }));
                            }}
                            onKeyDown={(e) => {
                              if (e.key !== "Enter" && e.key !== " ") return;
                              e.preventDefault();
                              e.stopPropagation();
                              updateWins([w.folderDir], (cur) => ({
                                ...cur,
                                windows: cur.windows.filter((x) => x.id !== w.id),
                              }));
                            }}
                            className="text-muted-foreground/50 hover:text-red-500"
                          >
                            ✕
                          </span>
                        )}
                      </button>
                    ),
                  )}
                  {activeFolderDir && (
                    <button
                      type="button"
                      onClick={() => void newWindow(activeFolderDir)}
                      title="New window around a fresh session"
                      className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-violet-500 hover:bg-accent/50"
                    >
                      <Plus className="size-3" /> window
                    </button>
                  )}
                  {activeFolderDir && (
                    <button
                      type="button"
                      onClick={() => void newSession(activeFolderDir)}
                      className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-violet-500 hover:bg-accent/50"
                      title={`New session in the focused folder (${shortcutHint("ab-new-session")} or ${shortcutHint("ab-new-terminal-right")})`}
                    >
                      <Plus className="size-3" /> session
                    </button>
                  )}
                  {selected && (
                    <button
                      type="button"
                      onClick={() => void closeSession(selected.sessionId)}
                      className="ml-auto shrink-0 rounded-md px-2 py-1 font-mono text-[10.5px] text-muted-foreground hover:bg-accent/50"
                      title={`Close session (${shortcutHint("ab-close-session")})`}
                      aria-label="Close the selected session"
                    >
                      Close {shortcutHint("ab-close-session")}
                    </button>
                  )}
                </div>
              )}

              {/* One flat pool of mounted terminals (never remounted — a remount
                  would respawn the shell). The active window's pane order assigns
                  each a percent-rect; panes in other windows stay hidden. */}
              <div ref={paneAreaRef} className="relative min-h-0 flex-1 overflow-hidden p-2">
                {(() => {
                  const panes: string[] = activeWin?.panes ?? [];
                  const liveCols =
                    colDrag && activeWin && colDrag.winId === activeWin.id
                      ? colDrag.cols
                      : activeWin?.cols;
                  const rects = paneRects(panes.length, liveCols);
                  const rectFor = (id: string) => {
                    const i = panes.indexOf(id);
                    return i < 0 ? undefined : rects[i];
                  };
                  return (
                    <>
                      {open.map((id) => {
                        const r = rectFor(id);
                        const s = sessionById.get(id);
                        return (
                          <div
                            key={id}
                            hidden={!r}
                            style={r ? paneStyle(r) : undefined}
                            className="absolute p-1.5"
                          >
                            <div
                              onClick={() =>
                                selectSession(folderOf.get(id)?.dir ?? cwds.current[id] ?? "", id)
                              }
                              className={cn(
                                "flex h-full flex-col overflow-hidden rounded-lg border bg-card",
                                // Amber (needs-you) wins the border over violet
                                // (focus) when both apply — see the folder-rail-ui
                                // skill's "Two accent hues" rule; class order here
                                // matters because `cn` (tailwind-merge) keeps only
                                // the last conflicting border-color utility.
                                focusedPaneId === id && "border-violet-500/60",
                                termAttention[id] && "border-amber-500/70",
                              )}
                            >
                              {s && (
                                <PaneHeader
                                  session={s}
                                  label={labelFor(s)}
                                  now={now}
                                  actions={actions}
                                  onUngroup={() => actions.ungroup(id)}
                                />
                              )}
                              {/* data-term-host marks terminal territory for the
                                  shortcut guard — keys typed here belong to the
                                  shell (Ctrl+D is EOF, not "new session"). */}
                              <div className="relative min-h-0 flex-1" data-term-host>
                                <TerminalView
                                  termId={id}
                                  cwd={folderOf.get(id)?.dir ?? cwds.current[id]}
                                  onExit={(exit) => handleExit(id, exit)}
                                  onTitle={onTitle}
                                />
                                {s && (
                                  <ColdCacheOverlay
                                    session={s}
                                    now={now}
                                    onCompact={() => actions.compactClaude(s)}
                                  />
                                )}
                              </div>
                            </div>
                          </div>
                        );
                      })}
                      {/* Diff panes: a folder's patch tiled beside its terminals. */}
                      {panes.filter(isDiffPane).map((id) => {
                        const r = rectFor(id);
                        const dir = diffPaneDir(id) ?? "";
                        return (
                          <div
                            key={id}
                            style={r ? paneStyle(r) : undefined}
                            className="absolute p-1.5"
                            onClick={() => setFocusedPaneId(id)}
                          >
                            <DiffPane
                              folder={folderByDir.get(dir)}
                              focused={focusedPaneId === id}
                              onClose={() => removePane(id)}
                            />
                          </div>
                        );
                      })}
                      {/* Files panes: a folder's full tree tiled beside its terminals. */}
                      {panes.filter(isFilesPane).map((id) => {
                        const r = rectFor(id);
                        const dir = filesPaneDir(id) ?? "";
                        return (
                          <div
                            key={id}
                            style={r ? paneStyle(r) : undefined}
                            className="absolute p-1.5"
                            onClick={() => setFocusedPaneId(id)}
                          >
                            <FolderFilesPane
                              folder={folderByDir.get(dir)}
                              focused={focusedPaneId === id}
                              openRequest={filesOpenRequests[dir]}
                              onClose={() => removePane(id)}
                            />
                          </div>
                        );
                      })}
                      {/* Preview panes: a folder's live dev server tiled beside
                          its terminals, with draw-on-page feedback. */}
                      {panes.filter(isPreviewPane).map((id) => {
                        const r = rectFor(id);
                        const dir = previewPaneDir(id) ?? "";
                        return (
                          <div
                            key={id}
                            style={r ? paneStyle(r) : undefined}
                            className="absolute p-1.5"
                            onClick={() => setFocusedPaneId(id)}
                          >
                            <PreviewPane
                              folder={folderByDir.get(dir)}
                              focused={focusedPaneId === id}
                              onClose={() => removePane(id)}
                            />
                          </div>
                        );
                      })}
                      {/* Tombstones: a shell that died on its own, holding the
                          slot it died in. The pane id says which kind this is,
                          so this pass can't overlap the terminal pass above —
                          a session is either its own id or its `~exit:` one,
                          never both. Dismissal is the only affordance;
                          reopening from the rail reclaims the slot. */}
                      {panes.filter(isExitPane).map((id) => {
                        const r = rectFor(id);
                        const sessionId = exitPaneSession(id) ?? "";
                        const s = sessionById.get(sessionId);
                        return (
                          <div
                            key={id}
                            style={r ? paneStyle(r) : undefined}
                            className="absolute p-1.5"
                            onClick={() => setFocusedPaneId(id)}
                          >
                            <PanePlaceholder
                              label={s ? labelFor(s) : "shell"}
                              detail={exitLabels[sessionId]}
                              tone="alert"
                              focused={focusedPaneId === id}
                              onRemove={() => removePane(id)}
                            />
                          </div>
                        );
                      })}
                      {/* Column dividers: drag to resize (snaps to thirds and
                          fifths), double-click for equal columns. Row layout
                          (≤3) has one per boundary; the ≥4 grid shares one
                          column boundary across rows. */}
                      {activeWin &&
                        panes.length >= 2 &&
                        (panes.length <= 3
                          ? rects.slice(1).map((r) => r.left)
                          : [rects[1].left]
                        ).map((x, i) => (
                          <div
                            key={`divider-${i}`}
                            role="separator"
                            aria-orientation="vertical"
                            aria-label="resize panes"
                            title="Drag to resize (snaps to thirds and fifths) — double-click for equal columns"
                            onPointerDown={(e) => startColDrag(e, activeWin, i)}
                            onDoubleClick={() => resetCols(activeWin)}
                            className="absolute top-0 z-10 h-full w-2 -translate-x-1/2 cursor-col-resize transition-colors hover:bg-violet-500/40 active:bg-violet-500/60"
                            style={{ left: `${x}%` }}
                          />
                        ))}
                      {panes.length === 0 && (
                        <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
                          <TerminalSquare className="size-10" />
                          <p className="text-sm">
                            {activeFolderDir
                              ? "No open panes — click a session in the rail to open it here."
                              : "Select a folder in the rail to see its sessions."}
                          </p>
                        </div>
                      )}
                    </>
                  );
                })()}
              </div>
            </div>
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>

      <CommandDialog
        open={splitOpen}
        onOpenChange={setSplitOpen}
        title="Add to window"
        description={
          activeFolder
            ? `Pick a session from ${activeFolder.name} to add as a pane.`
            : "Pick a session to add as a pane."
        }
        className="sm:max-w-lg"
      >
        <Command>
          <CommandInput autoFocus placeholder="Search sessions…" />
          <CommandList className="max-h-[60vh]">
            <CommandEmpty>No sessions match.</CommandEmpty>
            <CommandGroup heading="Sessions">
              {splitCandidates.map((s) => (
                <CommandItem
                  key={s.id}
                  value={sessionLabel(s)}
                  onSelect={() => {
                    setSplitOpen(false);
                    if (activeFolderDir) selectSession(activeFolderDir, s.id);
                  }}
                >
                  <TerminalSquare className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="flex-1 truncate">{sessionLabel(s)}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </CommandDialog>

      <AlertDialog
        open={confirmRemove != null}
        onOpenChange={closeOnFalse(() => setConfirmRemove(null))}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {confirmRemove?.label} from the rail?</AlertDialogTitle>
            <AlertDialogDescription>
              {confirmRemove?.sessionIds.length}{" "}
              {confirmRemove?.sessionIds.length === 1 ? "session is" : "sessions are"} still
              running. Removing will stop {confirmRemove?.sessionIds.length === 1 ? "it" : "them"}.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirmRemove) void performRemove(confirmRemove);
                setConfirmRemove(null);
              }}
            >
              Stop &amp; remove
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={confirmDeleteWt != null}
        onOpenChange={closeOnFalse(() => setConfirmDeleteWt(null))}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete worktree {confirmDeleteWt?.label}?</AlertDialogTitle>
            <AlertDialogDescription>
              Removes the checkout from disk (guarded — uncommitted changes, commits on no
              branch/remote, or a dev server still on its ports will stop it and tell you what to
              do). Its branch survives in the primary.
              {confirmDeleteWt && confirmDeleteWt.sessionIds.length > 0 && (
                <>
                  {" "}
                  {confirmDeleteWt.sessionIds.length}{" "}
                  {confirmDeleteWt.sessionIds.length === 1 ? "session is" : "sessions are"} still
                  running and will be stopped.
                </>
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirmDeleteWt) void performDeleteWorktree(confirmDeleteWt);
                setConfirmDeleteWt(null);
              }}
            >
              Delete worktree
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* The guards refused. Every reason gets its own row with its own way
          out, because the reasons are independent and only some are
          actionable from here: a stale dev server is one button, a dirty tree
          is work only the user can decide about. The force sits in the footer
          under a label that names what it discards — this dialog is where
          consent to lose work is actually given, since the confirm before it
          promised the delete was guarded. */}
      <AlertDialog
        open={blockedDelete != null}
        // Escape/cancel abandons the flow — except once the removal itself is
        // running, when "keep" can no longer be honored: the dialog stays up
        // (buttons locked) until the removal resolves and closes it honestly.
        onOpenChange={closeOnFalse(() => {
          if (!blockedRemovalInFlight) endDeleteFlow(blockedDeleteDir);
        })}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Can’t delete {blockedDelete?.name} yet</AlertDialogTitle>
            <AlertDialogDescription>
              The worktree is still on disk. Clear what’s below and it’ll delete cleanly, or delete
              anyway.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {/* Caveats about how the verdict itself was reached — chiefly a
              failed fetch, meaning these blockers were judged against stale
              refs. Above the list because it qualifies every row below it. */}
          {blockedDelete && blockedDelete.messages.length > 0 && (
            <ul className="flex flex-col gap-1 rounded-md border border-amber-500/40 bg-amber-500/10 px-2.5 py-2">
              {blockedDelete.messages.map((message) => (
                <li key={message} className="text-[11.5px] text-amber-600 dark:text-amber-400">
                  {message}
                </li>
              ))}
            </ul>
          )}
          <ul className="flex flex-col gap-3">
            {blockedDelete?.blockers.map((blocker, i) => {
              const port = stoppablePort(blocker);
              return (
                <li
                  key={`${blocker.kind}-${port ?? i}`}
                  className="flex items-start gap-3 rounded-lg border border-border bg-muted/40 p-3"
                >
                  <BlockerIcon kind={blocker.kind} losesWork={blocker.losesWork} />
                  <div className="flex min-w-0 flex-1 flex-col gap-1">
                    <span className="text-sm leading-snug">{blocker.message}</span>
                    <span className="text-xs leading-snug text-muted-foreground">
                      {blocker.remedy}
                    </span>
                  </div>
                  {port !== null && (
                    // Every action is disabled while any stop+retry runs, not
                    // just this row's: they all end in a removal of the same
                    // worktree, and two of those overlapping means concurrent
                    // `docker compose down` / `git worktree remove`.
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={deleteBusy}
                      onClick={() => void stopPortAndRetry(blockedDelete, port)}
                    >
                      {stoppingPort === port ? "Stopping…" : "Stop it"}
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
          <AlertDialogFooter>
            {/* Cancelling is real during a port stop (the retry checks the
                flow and stands down) but a lie once the removal is running,
                so only the removal locks it. */}
            <AlertDialogCancel disabled={blockedRemovalInFlight}>
              Keep the worktree
            </AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleteBusy}
              onClick={() => {
                if (blockedDelete) {
                  const target = blockedDelete.target;
                  endDeleteFlow(blockedDeleteDir);
                  void performDeleteWorktree(target, { force: true });
                }
              }}
            >
              {forceDeleteLabel(blockedDelete?.blockers ?? [])}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog open={startClaudeTarget != null} onOpenChange={closeOnFalse(commitStartClaude)}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>
              ✦ Start Claude{startClaudeTarget ? ` in ${startClaudeTarget.sessionName}` : ""}
            </DialogTitle>
          </DialogHeader>
          <Input
            autoFocus
            value={startClaudePrompt}
            onChange={(e) => setStartClaudePrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commitStartClaude();
              }
            }}
            placeholder="what are you working toward? (optional)"
          />
        </DialogContent>
      </Dialog>
    </div>
  );
}
