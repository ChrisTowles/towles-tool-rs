import { useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarClock,
  FolderGit2,
  FolderPlus,
  GitPullRequest,
  Plus,
  TerminalSquare,
} from "lucide-react";
import { fmtMins } from "@/components/agentboard-bits";
import { PaneHeader, WorkingContext } from "@/components/agentboard-pane";
import { RepoGroup, RollupChip } from "@/components/agentboard-rail";
import { DiffViewer } from "@/components/diff-view";
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
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  abInvoke,
  claudeCommand,
  claudeTitleName,
  ctxPct,
  isAgent,
  isCold,
  liveSessions,
  needsCompact,
  normalizeWins,
  paneRects,
  prForFolder,
  sessionLabel,
  sleep,
  termWrite,
  termWriteRetry,
  useAgentboardState,
  useNow,
  type AgentStatus,
  type FolderData,
  type Overlay,
  type PaneRect,
  type RemoveTarget,
  type RepoCandidate,
  type RepoData,
  type Selected,
  type SessionActions,
  type SessionData,
  type StartClaudeTarget,
  type WindowsPayload,
  windowColor,
} from "@/lib/agentboard";
import { fmtCountdown, useStoreSnapshot } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";
import { useWorkspace } from "@/lib/workspace";
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
  // Session awaiting the "what are you working toward?" prompt before Claude
  // actually launches — see `commitStartClaude`.
  const [startClaudeTarget, setStartClaudeTarget] = useState<StartClaudeTarget | null>(null);
  const [startClaudePrompt, setStartClaudePrompt] = useState("");
  // Session ids whose PTY is mounted (kept alive for scrollback), + their cwd.
  const [open, setOpen] = useState<string[]>([]);
  const cwds = useRef<Record<string, string>>({});
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
      void abInvoke("ab_save_collapsed", { key, collapsed: next });
      return { ...c, [key]: next };
    });
  }
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

  // Diff-preview dialog: a folder's full patch, fetched on demand.
  const [diff, setDiff] = useState<{ dir: string; name: string; text: string | null } | null>(
    null,
  );
  async function openDiff(dir: string, name: string) {
    setDiff({ dir, name, text: null });
    const text = await abInvoke<string>("ab_get_diff", { dir });
    setDiff((cur) => (cur && cur.dir === dir ? { ...cur, text: text ?? "" } : cur));
  }

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
    // the local copy is the live truth and only flows outward.
    if (wins === null && state.ts > 0) setWins(normalizeWins(state.windows));
  }, [wins, state.ts, state.windows]);

  function updateWins(folderDirs: string[], fn: (w: WindowsPayload) => WindowsPayload) {
    setWins((prev) => {
      const next = normalizeWins(fn(prev ?? { windows: [], activeWindows: {} }));
      for (const dir of folderDirs) dirtyWinFolders.current.add(dir);
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(() => {
        const touchedFolders = [...dirtyWinFolders.current];
        dirtyWinFolders.current = new Set();
        void abInvoke("ab_save_windows", { payload: next, touchedFolders });
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
  // "primary" — if the folder has none yet). A window may never span folders, so
  // this always targets the pane's own folder, never whatever window/folder
  // happened to be showing beforehand.
  function addPaneToActive(folderDir: string, sessionId: string) {
    updateWins([folderDir], (w) => {
      if (w.windows.some((win) => win.panes.includes(sessionId))) return w;
      let windows = w.windows;
      let windowId = w.activeWindows[folderDir];
      if (!windows.some((win) => win.id === windowId && win.folderDir === folderDir)) {
        windowId = `w${Date.now()}`;
        windows = [...windows, { id: windowId, name: "primary", folderDir, panes: [] }];
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
    // A pane lives in exactly one folder's window; find it before mutating
    // so we know which single folder to mark touched.
    const folderDir = wins?.windows.find((win) => win.panes.includes(sessionId))?.folderDir;
    updateWins(folderDir ? [folderDir] : [], (w) => ({
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
    ackFolder(folderDir);
  }

  function selectSession(folderDir: string, sessionId: string) {
    cwds.current[sessionId] = folderDir;
    setSelected({ folderDir, sessionId });
    setActiveFolderDir(folderDir);
    setOpen((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
    addPaneToActive(folderDir, sessionId);
    ackFolder(folderDir);
  }

  // The user is now looking at this folder's rail entry — clear its agents'
  // `unseen` flags (`sessionCatchesEye`'s pulse) via the backend tracker.
  function ackFolder(folderDir: string) {
    const name = folderNameByDir.get(folderDir);
    if (name) void abInvoke("ab_mark_seen", { name });
  }

  async function newSession(folderDir: string, launchClaude = false) {
    const rec = await abInvoke<SessionData>("ab_add_session", { dir: folderDir, name: null });
    if (!rec) return;
    selectSession(folderDir, rec.id);
    if (launchClaude) {
      setStartClaudeTarget({ folderDir, sessionId: rec.id, sessionName: rec.name, restart: false });
    }
  }

  // Actually launch Claude in `target`'s session, folding in whatever prompt
  // the user entered (or none) — see `commitStartClaude`, which reads the
  // dialog state and calls this.
  async function launchClaudeIn(target: StartClaudeTarget, prompt: string) {
    const { sessionId, sessionName, restart } = target;
    setOverlay(sessionId, "busy");
    const verb = restart ? "starting over — fresh Claude session" : "starting Claude";
    toast(prompt ? `✦ ${verb} in ${sessionName}: ${prompt}` : `✦ ${verb} in ${sessionName}`);
    if (prompt) void abInvoke("ab_set_session_purpose", { id: sessionId, text: prompt });
    if (restart) {
      await termWrite(sessionId, "\x03");
      await sleep(150);
      await termWrite(sessionId, "\x04");
      await sleep(300);
    }
    await termWriteRetry(sessionId, claudeCommand(prompt));
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
      setStartClaudeTarget({ folderDir, sessionId: s.id, sessionName: s.name, restart: false });
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
      setStartClaudeTarget({ folderDir, sessionId: s.id, sessionName: s.name, restart: true });
    },
    close: (sessionId) => void closeSession(sessionId),
    renameStart: setRenaming,
    ungroup: removePane,
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
          onClick: () => void openExternalUrl(p.url),
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
                    prs={snapshot.prs}
                    selected={selected}
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
                    onRemoveRepo={requestRemoveRepo}
                    onRenameCommit={commitRename}
                    onOpenDiff={openDiff}
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
            {activeFolder && activeRepo && (
              <WorkingContext
                repo={activeRepo}
                folder={activeFolder}
                pr={prForFolder(snapshot.prs, activeRepo.originUrl, activeFolder.branch)}
                onOpenDiff={openDiff}
              />
            )}
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
                          updateWins([w.folderDir], (cur) => ({
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
                ))}
                <button
                  type="button"
                  onClick={() =>
                    updateWins([activeFolderDir], (cur) => {
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
                                repo={repoOf.get(folderOf.get(id)?.dir ?? "")}
                                label={labelFor(s)}
                                now={now}
                                compactPct={state.compactRecommendPercent}
                                actions={actions}
                                onUngroup={() => actions.ungroup(id)}
                                onOpenDiff={openDiff}
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

      <Dialog
        open={startClaudeTarget != null}
        onOpenChange={(open) => {
          if (!open) commitStartClaude();
        }}
      >
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>✦ Start Claude{startClaudeTarget ? ` in ${startClaudeTarget.sessionName}` : ""}</DialogTitle>
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

      <Dialog open={diff != null} onOpenChange={(o) => !o && setDiff(null)}>
        <DialogContent className="flex h-[85vh] w-full flex-col sm:max-w-6xl">
          <DialogHeader>
            <DialogTitle className="flex items-baseline gap-3 font-mono text-sm">
              <span>{diff?.name}</span>
              <span className="text-[11px] font-normal text-muted-foreground">
                vs pushed base (merge-base with upstream, else origin/main)
              </span>
            </DialogTitle>
          </DialogHeader>
          {diff?.text == null ? (
            <p className="p-2 text-sm text-muted-foreground">Loading…</p>
          ) : (
            <DiffViewer text={diff.text} />
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
