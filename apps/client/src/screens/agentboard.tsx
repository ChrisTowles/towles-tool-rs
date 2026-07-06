import { useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarClock,
  ChevronDown,
  Folder,
  FolderGit2,
  FolderPlus,
  GitPullRequest,
  MoreVertical,
  Plus,
  TerminalSquare,
  Trash2,
} from "lucide-react";
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
import { Checkbox } from "@/components/ui/checkbox";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Slider } from "@/components/ui/slider";
import {
  abInvoke,
  agentRollup,
  claudeTitleName,
  ctxPct,
  type AgentStatus,
  fmtElapsed,
  isAgent,
  isCold,
  isSoloRepo,
  liveSessions,
  needsCompact,
  sessionLabel,
  sessionNeeds,
  sessionStatusText,
  statusColor,
  useAgentboardState,
  type FolderData,
  type RepoData,
  type SessionData,
  type StatePayload,
  type WindowsPayload,
  windowColor,
  windowOf,
} from "@/lib/agentboard";
import { toast } from "sonner";
import { fmtCountdown, useStoreSnapshot } from "@/lib/data";
import { useWorkspace } from "@/lib/workspace";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Write raw bytes into a session's PTY. False when the PTY isn't running. */
async function termWrite(termId: string, data: string): Promise<boolean> {
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
async function termWriteRetry(termId: string, data: string): Promise<boolean> {
  for (let i = 0; i < 20; i++) {
    if (await termWrite(termId, data)) return true;
    await sleep(150);
  }
  return false;
}

/** The lifecycle actions a session row can trigger. All are PTY writes — the
 * agent is whatever runs in the real shell, never a re-rendered proxy. */
type SessionActions = {
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
type PaneRect = { left: number; top: number; width: number; height: number };

function paneRects(n: number): PaneRect[] {
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
type Overlay = { status: AgentStatus; until: number };

type Selected = { folderDir: string; sessionId: string } | null;

/** Drop any `activeWindows` entries pointing at a window that no longer
 * exists (or whose folder no longer matches) — windows are created lazily
 * per folder as sessions open, so there's no "at least one window" floor. */
function normalizeWins(w: WindowsPayload): WindowsPayload {
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
function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

/** A repo row in the manage-repos picker (from `ab_discover_repos`): every
 * repo under the scan roots, unioned with every repo already on the rail. */
type RepoCandidate = { name: string; dir: string; active: boolean };

/** What a repo-remove confirmation (or immediate removal) needs to act on. */
type RemoveTarget = { label: string; dirs: string[]; sessionIds: string[] };

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
 * regrouping. Layout persists via debounced `ab_save_windows`. ⌘D = new
 * session in the selected folder, ⌘W = close the selected session.
 */
export function AgentboardScreen() {
  const state = useAgentboardState();
  const { snapshot } = useStoreSnapshot();
  const { openTab } = useWorkspace();
  const now = useNow(30_000);

  const [selected, setSelected] = useState<Selected>(null);
  // The folder whose windows the main area shows — set by clicking a folder
  // header or a session row. Null until the user picks a folder.
  const [activeFolderDir, setActiveFolderDir] = useState<string | null>(null);
  // Manage-repos picker: every repo under the scan roots + already on the
  // rail, fuzzy-searched by `repoQuery`, each toggled on/off by checkbox.
  const [addRepoOpen, setAddRepoOpen] = useState(false);
  const [repoQuery, setRepoQuery] = useState("");
  const [candidates, setCandidates] = useState<RepoCandidate[]>([]);
  // Pending remove awaiting confirmation because it would kill live sessions.
  const [confirmRemove, setConfirmRemove] = useState<RemoveTarget | null>(null);
  // Session ids whose PTY is mounted (kept alive for scrollback), + their cwd.
  const [open, setOpen] = useState<string[]>([]);
  const cwds = useRef<Record<string, string>>({});
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renamingWin, setRenamingWin] = useState<string | null>(null);
  // Live PTY window titles keyed by session id (Claude emits `✳ <title>`);
  // preferred over the backend label for sessions whose terminal is open.
  const [titles, setTitles] = useState<Record<string, string>>({});
  const onTitle = (id: string, title: string) =>
    setTitles((m) => (m[id] === title ? m : { ...m, [id]: title }));
  // The label to lead a session row/tab with: the live Claude terminal title
  // when present, else the backend-derived task/shell name.
  const labelFor = (s: SessionData) =>
    claudeTitleName(titles[s.id]) ?? sessionLabel(s);

  const repos = state.repos;

  // Index every session by id → its folder / its data, for cwd + pane chrome.
  const folderOf = useMemo(() => {
    const m = new Map<string, FolderData>();
    for (const r of repos)
      for (const f of r.folders) for (const s of f.sessions) m.set(s.id, f);
    return m;
  }, [repos]);
  const sessionById = useMemo(() => {
    const m = new Map<string, SessionData>();
    for (const r of repos) for (const f of r.folders) for (const s of f.sessions) m.set(s.id, s);
    return m;
  }, [repos]);

  // --- Window layout (Tier 5): frontend-owned, hydrated once, saved debounced.
  const [wins, setWins] = useState<WindowsPayload | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    // Hydrate from the first real payload (mock or ab_get_state); after that
    // the local copy is the live truth and only flows outward.
    if (wins === null && state.ts > 0) setWins(normalizeWins(state.windows));
  }, [wins, state.ts, state.windows]);

  function updateWins(fn: (w: WindowsPayload) => WindowsPayload) {
    setWins((prev) => {
      const next = normalizeWins(fn(prev ?? { windows: [], activeWindows: {} }));
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(() => {
        void abInvoke("ab_save_windows", { payload: next });
      }, 400);
      return next;
    });
  }

  // Windows belonging to the active folder, and whichever of those is focused.
  const windowsForFolder = useMemo(
    () => wins?.windows.filter((w) => w.folderDir === activeFolderDir) ?? [],
    [wins, activeFolderDir],
  );
  const activeWin =
    windowsForFolder.find((w) => w.id === (activeFolderDir && wins?.activeWindows[activeFolderDir])) ??
    windowsForFolder[0];

  // Add a session's pane to its own folder's focused window (creating one —
  // "main" — if the folder has none yet). A window may never span folders, so
  // this always targets the pane's own folder, never whatever window/folder
  // happened to be showing beforehand.
  function addPaneToActive(folderDir: string, sessionId: string) {
    updateWins((w) => {
      if (w.windows.some((win) => win.panes.includes(sessionId))) return w;
      let windows = w.windows;
      let windowId = w.activeWindows[folderDir];
      if (!windows.some((win) => win.id === windowId && win.folderDir === folderDir)) {
        windowId = `w${Date.now()}`;
        windows = [...windows, { id: windowId, name: "main", folderDir, panes: [] }];
      }
      return {
        windows: windows.map((win) =>
          win.id === windowId ? { ...win, panes: [...win.panes, sessionId] } : win,
        ),
        activeWindows: { ...w.activeWindows, [folderDir]: windowId },
      };
    });
  }

  function removePane(sessionId: string) {
    updateWins((w) => ({
      ...w,
      windows: w.windows.map((win) => ({
        ...win,
        panes: win.panes.filter((p) => p !== sessionId),
      })),
    }));
  }

  // Switch the main area to a folder without selecting one of its sessions
  // (clicking a folder header). Drops any selection from a *different*
  // folder so the cache bar / ⌘D / ⌘W / Close button never act on a session
  // that's no longer the one shown — a session selected in the folder you're
  // switching to stays selected.
  function selectFolder(folderDir: string) {
    setActiveFolderDir(folderDir);
    setSelected((cur) => (cur && cur.folderDir !== folderDir ? null : cur));
  }

  function selectSession(folderDir: string, sessionId: string) {
    cwds.current[sessionId] = folderDir;
    setSelected({ folderDir, sessionId });
    setActiveFolderDir(folderDir);
    setOpen((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
    addPaneToActive(folderDir, sessionId);
  }

  async function newSession(folderDir: string, launchClaude = false) {
    const rec = await abInvoke<SessionData>("ab_add_session", { dir: folderDir, name: null });
    if (!rec) return;
    selectSession(folderDir, rec.id);
    if (launchClaude) {
      setOverlay(rec.id, "busy");
      toast(`✦ starting Claude in ${rec.name}`);
      void termWriteRetry(rec.id, "claude\r");
    }
  }

  async function fetchCandidates(): Promise<RepoCandidate[]> {
    return (await abInvoke<RepoCandidate[]>("ab_discover_repos")) ?? [];
  }

  async function refreshCandidates() {
    setCandidates(await fetchCandidates());
  }

  // Add a repo to the rail; backend re-emits state so it appears. Mirrors
  // `ttr agentboard repos add <path>`.
  async function addRepoPath(dir: string) {
    const path = dir.trim();
    if (!path) return;
    await abInvoke("ab_add_repo", { path });
    await refreshCandidates();
  }

  // Actually remove: kill any live sessions first (killing a PTY is
  // client-mediated — see `closeSession`/`TerminalView`'s unmount effect),
  // then drop the checkout(s) from the watched list. Removes by `dir`, never
  // by resolved session name — a multi-checkout repo removes several dirs in
  // one batch, and `ab_remove_repo`'s name resolution shifts as each removal
  // changes the collision-disambiguated names of whatever's left.
  async function performRemove(target: RemoveTarget) {
    for (const id of target.sessionIds) await closeSession(id);
    for (const dir of target.dirs) await abInvoke("ab_remove_repo", { dir });
    await refreshCandidates();
  }

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
    // Never stack the confirm AlertDialog on top of the still-open
    // manage-repos CommandDialog — two simultaneous Radix dialogs fight over
    // the focus trap. Close the picker first; reopening isn't needed since
    // `performRemove` already refreshes `candidates` for next time.
    setAddRepoOpen(false);
    setConfirmRemove(target);
  }

  // Toggle a manage-repos row: add if off the rail, else remove it (guarded
  // by the same live-session confirmation as every other remove entry point).
  async function toggleRepo(c: RepoCandidate) {
    if (!c.active) {
      await addRepoPath(c.dir);
      return;
    }
    const folder = repos.flatMap((r) => r.folders).find((f) => f.dir === c.dir);
    requestRemoveRepo([c.dir], folder?.name ?? c.name);
  }

  // On opening the picker, (re)load the discoverable + on-rail repos.
  useEffect(() => {
    if (!addRepoOpen) return;
    setRepoQuery("");
    let active = true;
    void (async () => {
      const found = await fetchCandidates();
      if (active) setCandidates(found);
    })();
    return () => {
      active = false;
    };
  }, [addRepoOpen]);

  async function closeSession(sessionId: string) {
    await abInvoke("ab_close_session", { id: sessionId });
    setOpen((prev) => prev.filter((id) => id !== sessionId));
    setSelected((cur) => (cur?.sessionId === sessionId ? null : cur));
    removePane(sessionId);
  }

  async function commitRename(sessionId: string, name: string) {
    setRenaming(null);
    const trimmed = name.trim();
    if (trimmed)
      await abInvoke("ab_rename_session", { id: sessionId, name: trimmed });
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
      setOverlay(s.id, "busy");
      toast(`✦ starting Claude in ${s.name}`);
      void termWriteRetry(s.id, "claude\r");
    },
    stopClaude: (s) => {
      setOverlay(s.id, "interrupted");
      toast(`■ interrupting Claude — ${s.name}'s shell stays alive`);
      void (async () => {
        await termWrite(s.id, "\x03"); // interrupt the current turn
        await sleep(150);
        await termWrite(s.id, "\x04"); // Ctrl-D at the empty prompt exits Claude
      })();
    },
    compactClaude: (s) => {
      setOverlay(s.id, "busy");
      toast(`⤿ compacting ${s.name} — summarize & drop stale turns`);
      void termWrite(s.id, "/compact\r");
    },
    restartClaude: (folderDir, s) => {
      selectSession(folderDir, s.id);
      setOverlay(s.id, "busy");
      toast(`↻ starting over — fresh Claude session in ${s.name}`);
      void (async () => {
        await termWrite(s.id, "\x03");
        await sleep(150);
        await termWrite(s.id, "\x04");
        await sleep(300);
        await termWriteRetry(s.id, "claude\r"); // fresh session (not --continue)
      })();
    },
    close: (sessionId) => void closeSession(sessionId),
    renameStart: setRenaming,
    ungroup: removePane,
    focusWindow: (windowId) => {
      const win = wins?.windows.find((w) => w.id === windowId);
      if (!win) return;
      selectFolder(win.folderDir);
      updateWins((w) => ({
        ...w,
        activeWindows: { ...w.activeWindows, [win.folderDir]: windowId },
      }));
    },
  };

  // ⌘D = new session in the focused folder (matches the "+ session" button,
  // which only needs a focused folder, not a selected session); ⌘W = close
  // the selected session (needs an actual session, so it stays gated on
  // `selected`).
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      if (e.key === "d") {
        if (!activeFolderDir) return;
        e.preventDefault();
        void newSession(activeFolderDir);
      } else if (e.key === "w") {
        if (!selected) return;
        e.preventDefault();
        void closeSession(selected.sessionId);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeFolderDir, selected]);

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
      if (p.checks === "failing" || p.reviewState === "review_requested") {
        items.push({
          key: `pr:${p.repo}#${p.number}`,
          kind: "pr",
          title: `${p.repo.split("/").pop()} #${p.number}`,
          sub: p.checks === "failing" ? "Checks failing" : "Review requested",
          onClick: () => window.open(p.url, "_blank", "noopener"),
        });
      }
    }
    const soon = snapshot.events
      .filter((e) => e.startTs > now && e.startTs - now <= 30 * 60_000)
      .sort((a, b) => a.startTs - b.startTs)[0];
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
    <div className="flex h-full min-h-0">
      <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
        {/* Rail: rollup tally + header + attention strip + Repo → Folder → Session tree. */}
        <ResizablePanel defaultSize="360px" minSize="240px" maxSize="560px">
          <div className="flex h-full flex-col border-r">
            <RollupChip state={state} now={now} />
            <div className="flex items-center justify-between border-b px-3 py-2">
              <span className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Repos
              </span>
              <button
                type="button"
                onClick={() => setAddRepoOpen(true)}
                className="flex items-center gap-1 rounded-md px-1.5 py-1 text-xs font-medium text-violet-500 hover:bg-accent/50"
                title="Toggle which repos show up on the rail"
              >
                <FolderPlus className="size-3.5" /> Manage repos
              </button>
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
                      <span className="block truncate text-xs font-medium">
                        {a.title}
                      </span>
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
              <div className="flex flex-col">
                {repos.length === 0 && (
                  <div className="flex flex-col items-center gap-3 px-3 py-10 text-center">
                    <FolderGit2 className="size-8 text-muted-foreground" />
                    <p className="text-sm text-muted-foreground">
                      No repos on the rail yet.
                    </p>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => setAddRepoOpen(true)}
                    >
                      <FolderPlus className="size-3.5" /> Manage repos
                    </Button>
                  </div>
                )}
                {repos.map((repo) => (
                  <RepoGroup
                    key={repo.key}
                    repo={repo}
                    now={now}
                    compactPct={state.compactRecommendPercent}
                    selected={selected}
                    activeFolderDir={activeFolderDir}
                    collapsed={collapsed}
                    renaming={renaming}
                    titles={titles}
                    overlays={overlays}
                    wins={wins}
                    actions={actions}
                    onToggle={(k) => setCollapsed((c) => ({ ...c, [k]: !c[k] }))}
                    onSelectFolder={selectFolder}
                    onSelect={selectSession}
                    onNewSession={newSession}
                    onRemoveRepo={requestRemoveRepo}
                    onRenameCommit={commitRename}
                  />
                ))}
              </div>
            </ScrollArea>
          </div>
        </ResizablePanel>
        <ResizableHandle />

        {/* Main area: window strip + the active window's panes tiled side-by-side.
            Scoped to `activeFolderDir` — a window may only ever hold panes from
            the one folder it belongs to, so switching folders switches the
            whole strip, not just which panes happen to show. */}
        <ResizablePanel>
          <div className="flex h-full min-w-0 flex-col">
            {wins && activeFolderDir && (
              <div className="flex items-center gap-1 border-b bg-card px-2 py-1">
                {windowsForFolder.map((w) => (
                  <button
                    key={w.id}
                    type="button"
                    onClick={() => actions.focusWindow(w.id)}
                    onDoubleClick={() => setRenamingWin(w.id)}
                    title="double-click to rename"
                    className={cn(
                      "flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 text-[11px]",
                      w.id === activeWin?.id
                        ? "bg-accent text-foreground"
                        : "text-muted-foreground hover:bg-accent/50",
                    )}
                  >
                    <span className={cn("size-2 rounded-[3px]", windowColor(windowsForFolder, w.id))} />
                    {renamingWin === w.id ? (
                      <input
                        autoFocus
                        defaultValue={w.name}
                        onClick={(e) => e.stopPropagation()}
                        onBlur={(e) => {
                          const name = e.target.value.trim() || w.name;
                          setRenamingWin(null);
                          updateWins((cur) => ({
                            ...cur,
                            windows: cur.windows.map((x) => (x.id === w.id ? { ...x, name } : x)),
                          }));
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                          if (e.key === "Escape") setRenamingWin(null);
                        }}
                        className="w-24 rounded-sm border border-input bg-background px-1 text-[11px] outline-none"
                      />
                    ) : (
                      w.name
                    )}
                    <span className="font-mono text-[10px] text-muted-foreground/60">
                      {w.panes.length}⊞
                    </span>
                    {windowsForFolder.length > 1 && (
                      <span
                        role="button"
                        title="close window (panes ungroup; sessions stay in the rail)"
                        onClick={(e) => {
                          e.stopPropagation();
                          updateWins((cur) => ({
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
                ))}
                <button
                  type="button"
                  onClick={() =>
                    updateWins((cur) => {
                      const id = `w${Date.now()}`;
                      const count = cur.windows.filter((w) => w.folderDir === activeFolderDir).length;
                      return {
                        windows: [
                          ...cur.windows,
                          { id, name: `window ${count + 1}`, folderDir: activeFolderDir, panes: [] },
                        ],
                        activeWindows: { ...cur.activeWindows, [activeFolderDir]: id },
                      };
                    })
                  }
                  className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-violet-500 hover:bg-accent/50"
                >
                  <Plus className="size-3" /> window
                </button>
                {activeFolderDir && (
                  <button
                    type="button"
                    onClick={() => void newSession(activeFolderDir)}
                    className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-violet-500 hover:bg-accent/50"
                    title="New session in the focused folder (⌘D)"
                  >
                    <Plus className="size-3" /> session
                  </button>
                )}
                {selected && (
                  <button
                    type="button"
                    onClick={() => void closeSession(selected.sessionId)}
                    className="ml-auto shrink-0 rounded-md px-2 py-1 font-mono text-[10.5px] text-muted-foreground hover:bg-accent/50"
                    title="Close session (⌘W)"
                  >
                    Close ⌘W
                  </button>
                )}
              </div>
            )}

            {/* Focused agent's cache bar: ctx meter + cache state + lifecycle
                actions, prominent when it's time to compact (Calm Rail cachebar). */}
            {selected && (() => {
              const s = sessionById.get(selected.sessionId);
              const d = s?.agentState?.details;
              if (!s?.live || !isAgent(s) || !d?.contextUsed || !d.contextMax) return null;
              const pct = ctxPct(d);
              const cold = isCold(d, now);
              const nudge = needsCompact(d, now, state.compactRecommendPercent);
              const meterColor = nudge ? "bg-sky-500" : pct >= 70 ? "bg-yellow-500" : "bg-green-500";
              return (
                <div className="flex items-center gap-3 border-b bg-card/50 px-3 py-1.5 font-mono text-[11px] text-muted-foreground">
                  <span>ctx</span>
                  <span className="h-1.5 w-20 overflow-hidden rounded-full bg-accent">
                    <span
                      className={cn("block h-full", meterColor)}
                      style={{ width: `${Math.min(pct, 100)}%` }}
                    />
                  </span>
                  <span className={nudge ? "text-sky-500" : undefined}>{pct}%</span>
                  <span className="text-muted-foreground/40">·</span>
                  {cold ? (
                    <span className="text-sky-500">❄ cache cold</span>
                  ) : (
                    <span>
                      {d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"} cache warm ·{" "}
                      {fmtMins(d.cacheExpiresAt! - now)} left
                    </span>
                  )}
                  {nudge && (
                    <span className="ml-auto flex items-center gap-2">
                      <span className="rounded-md border border-sky-500/40 bg-sky-500/10 px-2 py-0.5 text-sky-500">
                        {pct}% & cold — resuming re-reads everything
                      </span>
                      <button
                        type="button"
                        onClick={() => actions.compactClaude(s)}
                        className="rounded-md border border-sky-500/40 px-2 py-0.5 text-sky-500 hover:bg-sky-500/10"
                      >
                        ⤿ compact
                      </button>
                      <button
                        type="button"
                        onClick={() => actions.restartClaude(selected.folderDir, s)}
                        className="rounded-md border border-border px-2 py-0.5 hover:bg-accent/50"
                      >
                        ↻ start over
                      </button>
                    </span>
                  )}
                </div>
              );
            })()}

            {/* One flat pool of mounted terminals (never remounted — a remount
                would respawn the shell). The active window's pane order assigns
                each a percent-rect; panes in other windows stay hidden. */}
            <div className="relative min-h-0 flex-1 overflow-hidden p-2">
              {(() => {
                const panes = activeWin?.panes ?? [];
                const rects = paneRects(panes.length);
                const rectFor = (id: string) => {
                  const i = panes.indexOf(id);
                  return i < 0 ? undefined : rects[i];
                };
                const paneStyle = (r: PaneRect) => ({
                  left: `${r.left}%`,
                  top: `${r.top}%`,
                  width: `${r.width}%`,
                  height: `${r.height}%`,
                });
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
                              "flex h-full flex-col overflow-hidden rounded-lg border bg-[#07090c]",
                              selected?.sessionId === id && "border-violet-500/60",
                            )}
                          >
                            {s && (
                              <PaneHeader
                                session={s}
                                folder={folderOf.get(id)}
                                label={labelFor(s)}
                                now={now}
                                compactPct={state.compactRecommendPercent}
                                actions={actions}
                                onUngroup={() => actions.ungroup(id)}
                              />
                            )}
                            <div className="min-h-0 flex-1">
                              <TerminalView
                                termId={id}
                                cwd={folderOf.get(id)?.dir ?? cwds.current[id]}
                                onExit={() => closeSession(id)}
                                onTitle={onTitle}
                              />
                            </div>
                          </div>
                        </div>
                      );
                    })}
                    {/* Panes restored from disk but not started this run. */}
                    {panes
                      .filter((id) => !open.includes(id))
                      .map((id) => {
                        const r = rectFor(id);
                        const s = sessionById.get(id);
                        const dir = folderOf.get(id)?.dir;
                        return (
                          <div key={id} style={r ? paneStyle(r) : undefined} className="absolute p-1.5">
                            <div className="flex h-full flex-col items-center justify-center gap-2 rounded-lg border border-dashed text-muted-foreground">
                              <span className="text-sm">{s ? labelFor(s) : "session"}</span>
                              {s && dir ? (
                                <div className="flex gap-3 font-mono text-xs">
                                  <button
                                    type="button"
                                    onClick={() => actions.start(dir, s)}
                                    className="hover:text-green-500"
                                  >
                                    ▶ shell
                                  </button>
                                  <button
                                    type="button"
                                    onClick={() => actions.startClaude(dir, s)}
                                    className="text-violet-500 hover:text-violet-400"
                                  >
                                    ✦ Claude
                                  </button>
                                  <button
                                    type="button"
                                    onClick={() => actions.ungroup(id)}
                                    className="hover:text-red-500"
                                  >
                                    ⊟ remove
                                  </button>
                                </div>
                              ) : (
                                <button
                                  type="button"
                                  onClick={() => actions.ungroup(id)}
                                  className="font-mono text-xs hover:text-red-500"
                                >
                                  session gone — ⊟ remove pane
                                </button>
                              )}
                            </div>
                          </div>
                        );
                      })}
                    {panes.length === 0 && (
                      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
                        <TerminalSquare className="size-10" />
                        <p className="text-sm">
                          {activeFolderDir
                            ? "Empty window — click a session in the rail to open it here."
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

      <CommandDialog
        open={addRepoOpen}
        onOpenChange={setAddRepoOpen}
        title="Manage repos"
        description="Toggle which repos show up on the rail."
        className="sm:max-w-2xl"
      >
        <Command>
          <CommandInput
            autoFocus
            value={repoQuery}
            onValueChange={setRepoQuery}
            placeholder="Search your git repos…"
          />
          <CommandList className="max-h-[60vh]">
            <CommandEmpty>
              {candidates.length === 0
                ? "No git repos found. Type an absolute path to add one."
                : "No match. Type an absolute path to add one."}
            </CommandEmpty>
            {candidates.length > 0 && (
              <CommandGroup heading="Repos">
                {candidates.map((c) => (
                  <CommandItem
                    key={c.dir}
                    value={`${c.name} ${c.dir}`}
                    onSelect={() => void toggleRepo(c)}
                  >
                    <Checkbox
                      checked={c.active}
                      tabIndex={-1}
                      className="pointer-events-none"
                    />
                    <span className="flex-1 truncate">{c.name}</span>
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {c.dir}
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            {repoQuery.startsWith("/") &&
              !candidates.some((c) => c.dir === repoQuery) && (
                <CommandGroup heading="Path">
                  <CommandItem
                    value={repoQuery}
                    onSelect={() => void addRepoPath(repoQuery)}
                  >
                    <FolderPlus className="size-3.5 shrink-0 text-violet-500" />
                    <span>Add path</span>
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {repoQuery}
                    </span>
                  </CommandItem>
                </CommandGroup>
              )}
          </CommandList>
        </Command>
      </CommandDialog>

      <AlertDialog
        open={confirmRemove != null}
        onOpenChange={(open) => {
          if (!open) setConfirmRemove(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {confirmRemove?.label} from the rail?</AlertDialogTitle>
            <AlertDialogDescription>
              {confirmRemove?.sessionIds.length}{" "}
              {confirmRemove?.sessionIds.length === 1 ? "session is" : "sessions are"} still
              running. Removing will stop{" "}
              {confirmRemove?.sessionIds.length === 1 ? "it" : "them"}.
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
    </div>
  );
}

/** One pane's chrome: glyph · dot · name · folder⎇branch · cache badge · ⊟. */
function PaneHeader({
  session,
  folder,
  label,
  now,
  compactPct,
  actions,
  onUngroup,
}: {
  session: SessionData;
  folder?: FolderData;
  label: string;
  now: number;
  compactPct: number;
  actions: SessionActions;
  onUngroup: () => void;
}) {
  const agent = isAgent(session) && session.live;
  const iconBtn = (label: string, title: string, onClick: () => void, hover: string) => (
    <button
      type="button"
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className={cn("font-mono text-xs text-muted-foreground/60", hover)}
    >
      {label}
    </button>
  );
  return (
    <div className="flex shrink-0 items-center gap-2 border-b bg-card px-2 py-1">
      <Glyph agent={isAgent(session)} />
      <Dot session={session} />
      <span className="truncate text-xs text-foreground">{label}</span>
      {folder && (
        <span className="truncate font-mono text-[10px] text-muted-foreground">
          {folder.name} ⎇ {folder.branch}
        </span>
      )}
      <span className="ml-auto flex shrink-0 items-center gap-2">
        <CacheBadge
          session={session}
          now={now}
          compactPct={compactPct}
          onCompact={() => actions.compactClaude(session)}
          long
        />
        {agent && iconBtn("■", "stop Claude (shell survives)", () => actions.stopClaude(session), "hover:text-red-500")}
        {iconBtn("⊟", "remove pane (session stays in the rail)", onUngroup, "hover:text-sky-500")}
        {iconBtn("✕", "kill session (PTY + record)", () => actions.close(session.id), "hover:text-red-500")}
      </span>
    </div>
  );
}

/** The board-wide agent tally pinned atop the rail: total + non-zero status
 * buckets + a ❄ compact count, with the Agentboard settings (compact
 * threshold) behind the trailing ⚙. Quiet when the board is at rest. */
function RollupChip({ state, now }: { state: StatePayload; now: number }) {
  const threshold = state.compactRecommendPercent;
  const r = agentRollup(state.repos, now, threshold);
  // Track the slider locally while dragging; commit on release.
  const [draft, setDraft] = useState<number | null>(null);
  const pct = draft ?? threshold;

  return (
    <div className="flex items-center gap-2.5 border-b bg-card px-3 py-2 font-mono text-[11px]">
      {r.total === 0 ? (
        <span className="text-muted-foreground/60">no agents running</span>
      ) : (
        <>
          <span className="text-foreground">
            {r.total} agent{r.total !== 1 && "s"}
          </span>
          {r.busy > 0 && <RollupBucket className="bg-yellow-500" n={r.busy} />}
          {r.waiting > 0 && <RollupBucket className="bg-blue-500" n={r.waiting} />}
          {r.error > 0 && <RollupBucket className="bg-red-500" n={r.error} />}
          {r.compact > 0 && (
            <span className="text-sky-500" title="cold sessions worth compacting">
              ❄{r.compact}
            </span>
          )}
        </>
      )}
      <Popover>
        <PopoverTrigger asChild>
          <button
            type="button"
            title="Agentboard settings"
            className="ml-auto text-muted-foreground/60 hover:text-foreground"
          >
            ⚙
          </button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-72">
          <div className="flex flex-col gap-3">
            <div className="text-sm font-medium">Agentboard settings</div>
            <div className="text-xs text-muted-foreground">
              Recommend compacting a cold session at or above{" "}
              <span className="font-mono text-sky-500">{pct}%</span> context.
            </div>
            <Slider
              min={10}
              max={90}
              step={5}
              value={[pct]}
              onValueChange={([v]) => setDraft(v)}
              onValueCommit={([v]) => {
                setDraft(null);
                void abInvoke("ab_set_compact_percent", { percent: v });
              }}
            />
            <div className="text-[11px] text-muted-foreground/70">
              Past this threshold, a session whose prompt cache expired shows the ❄ compact
              nudge. Stored in the shared towles-tool settings file.
            </div>
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}

function RollupBucket({ className, n }: { className: string; n: number }) {
  return (
    <span className="flex items-center gap-1 text-muted-foreground">
      <span className={cn("size-1.5 rounded-full", className)} />
      {n}
    </span>
  );
}

/** ✦ for an agent session, ❯ for a plain shell. */
function Glyph({ agent }: { agent: boolean }) {
  return (
    <span
      className={cn(
        "w-4 shrink-0 text-center font-mono text-xs",
        agent ? "text-violet-500" : "text-muted-foreground",
      )}
    >
      {agent ? "✦" : "❯"}
    </span>
  );
}

/** Status dot mirroring `statusColor`; pulses while busy. A session with no
 * live PTY shows a hollow ring — the record exists but nothing is running. */
function Dot({ session }: { session: SessionData }) {
  if (!session.live) {
    return (
      <span className="size-2 shrink-0 rounded-full border-[1.5px] border-muted-foreground/50 bg-transparent" />
    );
  }
  const st = session.agentState?.status;
  return (
    <span
      className={cn(
        "size-2 shrink-0 rounded-full",
        st ? statusColor(st) : "bg-muted-foreground/40",
        st === "busy" && "animate-pulse",
      )}
    />
  );
}

function RepoGroup({
  repo,
  now,
  compactPct,
  selected,
  activeFolderDir,
  collapsed,
  renaming,
  titles,
  overlays,
  wins,
  actions,
  onToggle,
  onSelectFolder,
  onSelect,
  onNewSession,
  onRemoveRepo,
  onRenameCommit,
}: {
  repo: RepoData;
  now: number;
  compactPct: number;
  selected: Selected;
  activeFolderDir: string | null;
  collapsed: Record<string, boolean>;
  renaming: string | null;
  titles: Record<string, string>;
  overlays: Record<string, Overlay>;
  wins: WindowsPayload | null;
  actions: SessionActions;
  onToggle: (key: string) => void;
  onSelectFolder: (folderDir: string) => void;
  onSelect: (folderDir: string, sessionId: string) => void;
  onNewSession: (folderDir: string, launchClaude?: boolean) => void;
  onRemoveRepo: (dirs: string[], label: string) => void;
  onRenameCommit: (sessionId: string, name: string) => void;
}) {
  const solo = isSoloRepo(repo);

  const sessionRows = (folder: FolderData) =>
    folder.sessions.length === 0 ? (
      <div className="flex items-center gap-2.5 py-1 pr-3 pl-9 text-[11px] italic text-muted-foreground/60">
        no sessions
        <button
          type="button"
          onClick={() => onNewSession(folder.dir, true)}
          className="not-italic text-violet-500 hover:underline"
        >
          ✦ start Claude
        </button>
        <span className="text-muted-foreground/40">·</span>
        <button
          type="button"
          onClick={() => onNewSession(folder.dir, false)}
          className="not-italic text-violet-500 hover:underline"
        >
          + shell
        </button>
      </div>
    ) : (
      folder.sessions.map((s) => (
        <SessionRow
          key={s.id}
          session={s}
          folderDir={folder.dir}
          now={now}
          compactPct={compactPct}
          title={titles[s.id]}
          active={selected?.sessionId === s.id}
          renaming={renaming === s.id}
          overlay={overlays[s.id]}
          wins={wins}
          actions={actions}
          onSelect={() => onSelect(folder.dir, s.id)}
          onRenameCommit={(name) => onRenameCommit(s.id, name)}
        />
      ))
    );

  // Solo repo: collapse repo + folder into one header (repo · branch).
  if (solo) {
    const folder = repo.folders[0];
    const isCollapsed = collapsed[repo.key];
    return (
      <div className="border-b">
        <FolderHeader
          scope="repo"
          title={repo.name}
          path={folder.dir}
          branch={folder.branch}
          isWorktree={folder.isWorktree}
          filesChanged={folder.filesChanged}
          linesAdded={folder.linesAdded}
          linesRemoved={folder.linesRemoved}
          commitsDelta={folder.commitsDelta}
          progressPercent={folder.metadata?.progress?.percent}
          needs={repo.needs}
          collapsed={isCollapsed}
          active={activeFolderDir === folder.dir}
          onToggle={() => {
            onToggle(repo.key);
            onSelectFolder(folder.dir);
          }}
          onNewSession={() => onNewSession(folder.dir)}
          onRemoveRepo={() => onRemoveRepo([folder.dir], repo.name)}
        />
        {!isCollapsed && (
          <div className="group pb-2">
            <PurposeRow folder={folder} />
            {sessionRows(folder)}
          </div>
        )}
      </div>
    );
  }

  // Multi-checkout repo: repo header, then each folder as a sub-header.
  const repoCollapsed = collapsed[repo.key];
  return (
    <div className="border-b">
      <div className="sticky top-0 z-10 flex w-full items-center gap-2 bg-card px-3 py-2 hover:bg-accent/50">
        <button
          type="button"
          onClick={() => onToggle(repo.key)}
          className="flex min-w-0 flex-1 items-center gap-2"
        >
          <Chevron collapsed={repoCollapsed} />
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate text-sm font-semibold">{repo.name}</span>
          {repo.needs > 0 && <NeedsBadge n={repo.needs} className="ml-auto" />}
        </button>
        <RepoMenu
          onRemove={() =>
            onRemoveRepo(
              repo.folders.map((f) => f.dir),
              repo.name,
            )
          }
        />
      </div>
      {!repoCollapsed &&
        repo.folders.map((folder) => {
          const key = `${repo.key}::${folder.dir}`;
          const fCollapsed = collapsed[key];
          return (
            <div key={folder.dir}>
              <FolderHeader
                scope="folder"
                title={folder.name}
                path={folder.dir}
                branch={folder.branch}
                isWorktree={folder.isWorktree}
                filesChanged={folder.filesChanged}
                linesAdded={folder.linesAdded}
                linesRemoved={folder.linesRemoved}
                commitsDelta={folder.commitsDelta}
                progressPercent={folder.metadata?.progress?.percent}
                needs={folder.needs}
                collapsed={fCollapsed}
                active={activeFolderDir === folder.dir}
                onToggle={() => {
                  onToggle(key);
                  onSelectFolder(folder.dir);
                }}
                onNewSession={() => onNewSession(folder.dir)}
                onRemoveRepo={() => onRemoveRepo([folder.dir], folder.name)}
              />
              {!fCollapsed && (
                <div className="group pb-1">
                  <PurposeRow folder={folder} />
                  {sessionRows(folder)}
                </div>
              )}
            </div>
          );
        })}
    </div>
  );
}

/** The folder's user-authored purpose: a faint one-liner under the header.
 * Click to edit inline (Enter saves, Esc cancels; blank clears). When unset,
 * the row takes up no space at rest — the "+ purpose" hint only appears (and
 * only then claims a row) while hovering the folder group, so a resting rail
 * with many empty folders doesn't pad itself out with blank lines. */
function PurposeRow({ folder }: { folder: FolderData }) {
  const [editing, setEditing] = useState(false);
  const purpose = folder.purpose?.trim() ?? "";

  async function commit(text: string) {
    setEditing(false);
    const trimmed = text.trim();
    if (trimmed === purpose) return;
    await abInvoke("ab_set_folder_purpose", { dir: folder.dir, text: trimmed || null });
  }

  if (editing) {
    return (
      <div className="py-0.5 pr-3 pl-9">
        <input
          autoFocus
          defaultValue={purpose}
          placeholder="what are you working toward here?"
          onBlur={(e) => void commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void commit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") setEditing(false);
          }}
          className="w-full rounded-sm border border-input bg-background px-1.5 py-0.5 text-[11px] outline-none"
        />
      </div>
    );
  }

  if (!purpose) {
    return (
      <button
        type="button"
        onClick={() => setEditing(true)}
        title="Edit folder purpose"
        className="hidden w-full truncate py-0.5 pr-3 pl-9 text-left text-[11px] text-muted-foreground/50 group-hover:block"
      >
        + what are you working toward here?
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => setEditing(true)}
      title="Edit folder purpose"
      className="block w-full truncate py-0.5 pr-3 pl-9 text-left text-[11px] text-muted-foreground hover:text-foreground"
    >
      {purpose}
    </button>
  );
}

function FolderHeader({
  scope,
  title,
  path,
  branch,
  isWorktree,
  filesChanged,
  linesAdded,
  linesRemoved,
  commitsDelta,
  progressPercent,
  needs,
  collapsed,
  active,
  onToggle,
  onNewSession,
  onRemoveRepo,
}: {
  scope: "repo" | "folder";
  title: string;
  /** Full path on disk, shown in the kebab menu. */
  path: string;
  branch: string;
  isWorktree: boolean;
  filesChanged: number;
  linesAdded: number;
  linesRemoved: number;
  commitsDelta: number;
  progressPercent?: number | null;
  needs: number;
  collapsed: boolean;
  /** Whether this folder is the one currently shown in the main pane area. */
  active: boolean;
  onToggle: () => void;
  onNewSession: () => void;
  onRemoveRepo?: () => void;
}) {
  return (
    <div
      className={cn(
        "flex items-center gap-2 bg-card px-3 py-2 hover:bg-accent/50",
        scope === "repo" ? "sticky top-0 z-10" : "pl-6",
        active && "bg-accent/60",
      )}
    >
      <button
        type="button"
        onClick={onToggle}
        className="flex min-w-0 flex-1 items-center gap-2"
      >
        <Chevron collapsed={collapsed} />
        {scope === "repo" ? (
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <Folder className="size-3.5 shrink-0 text-muted-foreground/70" />
        )}
        <span className="shrink-0 font-mono text-sm text-muted-foreground/60">
          {isWorktree ? "w/" : "p/"}
        </span>
        <span
          className={cn(
            "min-w-0 truncate",
            scope === "repo"
              ? "text-sm font-semibold"
              : "text-sm text-muted-foreground",
          )}
        >
          {title}
        </span>
        <span className="min-w-0 truncate font-mono text-[11px] text-muted-foreground">
          ⎇ {branch}
        </span>
        {(linesAdded > 0 || linesRemoved > 0) && (
          <span
            className="flex shrink-0 items-center gap-1 font-mono text-[11px]"
            title={`${filesChanged} file${filesChanged === 1 ? "" : "s"} changed, ${commitsDelta} commit${commitsDelta === 1 ? "" : "s"} ahead`}
          >
            {linesAdded > 0 && <span className="text-green-500">+{linesAdded}</span>}
            {linesRemoved > 0 && <span className="text-red-500">−{linesRemoved}</span>}
          </span>
        )}
        {typeof progressPercent === "number" && (
          <span className="shrink-0 rounded-md border border-violet-500/40 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500">
            {Math.round(progressPercent)}%
          </span>
        )}
      </button>
      {needs > 0 && <NeedsBadge n={needs} />}
      <button
        type="button"
        onClick={onNewSession}
        className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-violet-500"
        title="New session (⌘D)"
      >
        <Plus className="size-3.5" />
      </button>
      {onRemoveRepo && <RepoMenu path={path} onRemove={onRemoveRepo} />}
    </div>
  );
}

/** Kebab menu on a repo header: shows the full folder path, plus "Remove from rail". */
function RepoMenu({ path, onRemove }: { path?: string; onRemove: () => void }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground"
        title="Repo actions"
      >
        <MoreVertical className="size-3.5" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-auto min-w-56">
        {path && (
          <>
            <DropdownMenuLabel className="font-mono text-[11px] font-normal whitespace-nowrap text-muted-foreground">
              {path}
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
          </>
        )}
        <DropdownMenuItem variant="destructive" onSelect={onRemove} className="whitespace-nowrap">
          <Trash2 className="size-3.5" /> Remove from rail
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function SessionRow({
  session,
  folderDir,
  now,
  compactPct,
  title,
  active,
  renaming,
  overlay,
  wins,
  actions,
  onSelect,
  onRenameCommit,
}: {
  session: SessionData;
  folderDir: string;
  now: number;
  compactPct: number;
  title?: string;
  active: boolean;
  renaming: boolean;
  overlay?: Overlay;
  wins: WindowsPayload | null;
  actions: SessionActions;
  onSelect: () => void;
  onRenameCommit: (name: string) => void;
}) {
  // Apply the optimistic lifecycle overlay (start/stop just happened) until
  // the watcher's next scan delivers ground truth.
  const eff: SessionData =
    overlay && overlay.until > Date.now()
      ? {
          ...session,
          live: true,
          agentState: {
            agent: "claude-code",
            session: "",
            ts: now,
            ...session.agentState,
            status: overlay.status,
          },
        }
      : session;
  const needs = sessionNeeds(eff);
  const agent = isAgent(eff);
  const grouped = wins ? windowOf(wins.windows, session.id) : undefined;
  // Prefer the live Claude terminal title (`✳ <title>`) when the PTY is open.
  const label = claudeTitleName(title) ?? sessionLabel(eff);
  // Hover-reveal is driven by JS state, not CSS `:hover` — the Tauri webview's
  // WebKitGTK doesn't reliably update `:hover` on real pointer movement, so
  // `group-hover` utilities never fire even though `matchMedia('(hover:
  // hover)')` reports true.
  const [hovered, setHovered] = useState(false);
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onDoubleClick={() => actions.renameStart(session.id)}
      onKeyDown={(e) => e.key === "Enter" && onSelect()}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      className={cn(
        "ml-1.5 flex cursor-pointer items-center gap-2.5 border-l-2 border-transparent py-1.5 pr-3 pl-7",
        hovered && "bg-accent/50",
        active && "border-l-violet-500 bg-accent",
        needs && "border-l-amber-500",
      )}
    >
      <Glyph agent={agent} />
      <Dot session={eff} />
      {renaming ? (
        <input
          autoFocus
          defaultValue={session.name}
          onClick={(e) => e.stopPropagation()}
          onBlur={(e) => onRenameCommit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter")
              onRenameCommit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") onRenameCommit(session.name);
          }}
          className="min-w-0 flex-1 rounded-sm border border-input bg-background px-1 text-sm outline-none"
        />
      ) : (
        <>
          <span
            className={cn(
              "min-w-0 flex-1 truncate",
              eff.live ? "text-foreground" : "text-muted-foreground",
            )}
          >
            {label}
          </span>
          {label !== session.name && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70">
              {session.name}
            </span>
          )}
          {!agent && eff.shellKind && (
            <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground/50">
              {eff.shellKind}
            </span>
          )}
          {grouped && (
            <span
              role="button"
              title={`in window “${grouped.name}” — click to focus it`}
              onClick={(e) => {
                e.stopPropagation();
                actions.focusWindow(grouped.id);
              }}
              className={cn(
                "flex min-w-0 shrink items-center gap-1",
                (active || hovered) && "hidden",
              )}
            >
              <span
                className={cn(
                  "size-2 shrink-0 rounded-[3px]",
                  windowColor(
                    wins?.windows.filter((w) => w.folderDir === grouped.folderDir) ?? [],
                    grouped.id,
                  ),
                )}
              />
              <span className="max-w-12 truncate font-mono text-[9.5px] text-muted-foreground/60">
                {grouped.name}
              </span>
            </span>
          )}
          {/* Resting: cache + status. Hovered or selected: the lifecycle controls. */}
          <span
            className={cn(
              "ml-auto flex min-w-0 shrink items-center gap-2",
              (active || hovered) && "hidden",
            )}
          >
            <CacheBadge
              session={eff}
              now={now}
              compactPct={compactPct}
              onCompact={() => actions.compactClaude(eff)}
            />
            {eff.live && (
              <span
                className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70"
                title="running for"
              >
                {fmtElapsed(now - eff.createdAt)}
              </span>
            )}
            <span className="min-w-0 truncate text-[11px] text-muted-foreground">
              {sessionStatusText(eff)}
            </span>
          </span>
          <span
            className={cn(
              "ml-auto shrink-0 items-center gap-2",
              active || hovered ? "flex" : "hidden",
            )}
          >
            <RowControls session={eff} folderDir={folderDir} grouped={!!grouped} actions={actions} />
          </span>
          {needs && (
            <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />
          )}
        </>
      )}
    </div>
  );
}

/** Hover-reveal lifecycle controls for a session row. Which buttons show
 * depends on the session's state:
 *   not started → ▶ shell · ✦ Claude
 *   live shell  → ✦ Claude
 *   live agent  → ■ stop · ⤿ compact (at prompt) · ↻ restart
 * plus ✎ rename and ✕ close, always. */
function RowControls({
  session,
  folderDir,
  grouped,
  actions,
}: {
  session: SessionData;
  folderDir: string;
  grouped: boolean;
  actions: SessionActions;
}) {
  const agent = isAgent(session);
  const st = session.agentState?.status;
  // `/compact` only lands when Claude is at its prompt, not mid-turn.
  const atPrompt = st === "waiting" || st === "idle" || st === "complete";
  const btn = (
    label: string,
    title: string,
    onClick: () => void,
    className = "text-muted-foreground hover:text-foreground",
  ) => (
    <button
      type="button"
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      className={cn("w-4 text-center font-mono text-xs", className)}
    >
      {label}
    </button>
  );

  return (
    <>
      {!session.live && btn("▶", "start shell", () => actions.start(folderDir, session), "text-muted-foreground hover:text-green-500")}
      {(!session.live || !agent) &&
        btn("✦", "start Claude here", () => actions.startClaude(folderDir, session), "text-violet-500 hover:text-violet-400")}
      {session.live && agent && (
        <>
          {btn("■", "stop Claude (shell survives)", () => actions.stopClaude(session), "text-muted-foreground hover:text-red-500")}
          {atPrompt && btn("⤿", "compact context (/compact)", () => actions.compactClaude(session), "text-muted-foreground hover:text-sky-500")}
          {btn("↻", "start over — fresh Claude session", () => actions.restartClaude(folderDir, session), "text-muted-foreground hover:text-orange-500")}
        </>
      )}
      {grouped &&
        btn("⊟", "ungroup — remove pane from its window", () => actions.ungroup(session.id), "text-muted-foreground hover:text-sky-500")}
      {btn("✎", "rename", () => actions.renameStart(session.id))}
      {btn("✕", "close session", () => actions.close(session.id), "text-muted-foreground hover:text-red-500")}
    </>
  );
}

/** Context/cache health for a live agent session, in the row's meta cluster.
 * Quiet mono text: `41% ◔4m` while warm (⧗ for a 1h cache), `41% ❄` when cold,
 * and an ice-washed `❄ 63% compact` pill when cold at/over the threshold. */
function CacheBadge({
  session,
  now,
  compactPct,
  onCompact,
  long = false,
}: {
  session: SessionData;
  now: number;
  compactPct: number;
  /** When set, the ❄ compact pill is clickable and runs /compact directly. */
  onCompact?: () => void;
  /** Long form spells out "compact"; the rail uses the short `❄ N%`. */
  long?: boolean;
}) {
  const d = session.agentState?.details;
  if (!session.live || !d?.contextUsed || !d.contextMax) return null;
  const pct = ctxPct(d);
  const cold = isCold(d, now);

  if (needsCompact(d, now, compactPct)) {
    const pill = "shrink-0 rounded-md border border-sky-500/50 bg-sky-500/10 px-1.5 font-mono text-[10.5px] text-sky-500";
    const hint = `${pct}% of context used and the prompt cache expired — resuming re-reads everything.`;
    return onCompact ? (
      <button
        type="button"
        title={`${hint} Click to /compact.`}
        onClick={(e) => {
          e.stopPropagation();
          onCompact();
        }}
        className={cn(pill, "hover:bg-sky-500/20")}
      >
        ❄ {pct}%{long && " compact"}
      </button>
    ) : (
      <span title={`${hint} Consider /compact or a fresh session.`} className={pill}>
        ❄ {pct}%{long && " compact"}
      </span>
    );
  }

  const warmth = cold ? "❄" : `${d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"}${fmtMins(d.cacheExpiresAt! - now)}`;
  return (
    <span
      title={cold ? "prompt cache expired" : "prompt cache warm — time left"}
      className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70"
    >
      {pct}% {warmth}
    </span>
  );
}

/** Millis → whole minutes for the cache countdown, floored at 1 ("<1m" ≈ 1m). */
function fmtMins(ms: number): string {
  return `${Math.max(1, Math.round(ms / 60_000))}m`;
}

function NeedsBadge({ n, className }: { n: number; className?: string }) {
  return (
    <span
      className={cn(
        "shrink-0 rounded-md border border-amber-500/50 bg-amber-500/10 px-1.5 font-mono text-[10.5px] text-amber-500",
        className,
      )}
    >
      {n} ⚑
    </span>
  );
}

function Chevron({ collapsed }: { collapsed: boolean }) {
  return (
    <ChevronDown
      className={cn(
        "size-3.5 shrink-0 text-muted-foreground transition-transform",
        collapsed && "-rotate-90",
      )}
    />
  );
}
