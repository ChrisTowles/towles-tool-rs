import { useState } from "react";
import {
  CircleDot,
  Folder,
  FolderGit2,
  MoreVertical,
  PanelLeftOpen,
  Plus,
  StickyNote,
  Trash2,
} from "lucide-react";
import {
  AgentStatusLine,
  AheadBehind,
  CacheBadge,
  Chevron,
  DiffButton,
  Dot,
  DotCount,
  Glyph,
  IconBtn,
  NeedsBadge,
  PrChip,
  PurposeRow,
  WorktreeBadge,
} from "@/components/agentboard-bits";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";
import {
  abCreateIssue,
  abInvoke,
  agentRollup,
  claudeTitleName,
  fmtElapsed,
  isAgent,
  isSoloRepo,
  pathScope,
  prForFolder,
  sessionCatchesEye,
  sessionLabel,
  sessionStatusText,
  windowColor,
  windowOf,
  type AgWindow,
  type FolderData,
  type Overlay,
  type RepoData,
  type SessionActions,
  type SessionData,
  type StatePayload,
  type WindowsPayload,
} from "@/lib/agentboard";
import { toast } from "sonner";
import type { PrItem } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";

/** Ambient status color for a set of sessions hidden behind a collapse:
 * red if one errored, blue if one is waiting on you, yellow while an agent is
 * busy, else a calm emerald for "live but idle". Null when nothing is live. */
function collapsedLiveColor(sessions: SessionData[]): string | null {
  const live = sessions.filter((s) => s.live);
  if (live.length === 0) return null;
  if (live.some((s) => s.agentState?.status === "error")) return "bg-red-500";
  if (live.some((s) => s.agentState?.status === "waiting")) return "bg-blue-500";
  if (live.some((s) => s.agentState?.status === "busy")) return "bg-yellow-500";
  return "bg-emerald-500";
}

/** Shown on a collapsed folder/repo header: a colored dot + count telling you
 * running sessions are hidden inside (so a collapsed folder doesn't look
 * asleep when agents are working in it). Nothing when nothing is live. */
function CollapsedLive({ sessions }: { sessions: SessionData[] }) {
  const color = collapsedLiveColor(sessions);
  if (!color) return null;
  const n = sessions.filter((s) => s.live).length;
  return (
    <span
      className="flex shrink-0 items-center gap-1"
      title={`${n} running session${n > 1 ? "s" : ""} hidden — expand to see`}
    >
      <span className={cn("size-2 rounded-full", color)} />
      <span className="font-mono text-[10px] text-muted-foreground/70">{n}</span>
    </span>
  );
}

/** The whole rail collapsed to a narrow icon strip: an expand toggle, a live
 * session tally, then one icon per checkout (FolderGit2 for a solo repo,
 * Folder per checkout of a multi-checkout repo, repos separated by hairlines).
 * Each icon keeps the signals a collapsed folder header shows — the ambient
 * live-status dot and the amber needs-you count — so collapsing the rail
 * never hides work waiting on you. Clicking an icon focuses that folder. */
export function RailIconStrip({
  repos,
  activeFolderDir,
  attentionCount,
  onSelectFolder,
  onExpand,
  expandHint,
}: {
  repos: RepoData[];
  activeFolderDir: string | null;
  /** Items in the rail's attention strip (failing PRs, imminent meeting) —
   * hidden while collapsed, so the strip surfaces the count instead. */
  attentionCount: number;
  onSelectFolder: (dir: string) => void;
  onExpand: () => void;
  /** Keyboard hint for the expand tooltip, e.g. "⌘⇧B". */
  expandHint: string;
}) {
  const allSessions = repos.flatMap((r) => r.folders.flatMap((f) => f.sessions));
  const liveColor = collapsedLiveColor(allSessions);
  const liveN = allSessions.filter((s) => s.live).length;

  const folderIcon = (repo: RepoData, folder: FolderData, solo: boolean) => {
    const active = folder.dir === activeFolderDir;
    const needs = solo ? repo.needs : folder.needs;
    const live = collapsedLiveColor(folder.sessions);
    const label = solo ? repo.name : `${repo.name} / ${folder.name}`;
    return (
      <Tooltip key={folder.dir}>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label={label}
            aria-current={active || undefined}
            onClick={() => onSelectFolder(folder.dir)}
            className={cn(
              "relative flex size-9 shrink-0 items-center justify-center rounded-md border-l-2 border-transparent text-muted-foreground hover:bg-accent/50",
              active && "border-l-violet-500 bg-accent text-foreground",
              // Attention outranks focus on the accent edge (folder-rail rule).
              needs > 0 && "border-l-amber-500",
            )}
          >
            {solo ? <FolderGit2 className="size-4" /> : <Folder className="size-4" />}
            {live && <span className={cn("absolute top-1 right-1 size-2 rounded-full", live)} />}
            {needs > 0 && (
              <span className="absolute -right-1 -bottom-1 min-w-4 rounded-full border border-amber-500/50 bg-background px-0.5 text-center font-mono text-[9px] leading-[14px] text-amber-500">
                {needs}
              </span>
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent side="right">
          {label} — ⎇ {folder.branch}
          {needs > 0 && ` · ${needs} need${needs === 1 ? "s" : ""} you`}
        </TooltipContent>
      </Tooltip>
    );
  };

  return (
    <div className="flex h-full w-12 shrink-0 flex-col items-center border-r bg-background py-2">
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label="Expand the folder rail"
            onClick={onExpand}
            className="flex size-8 items-center justify-center rounded-md text-muted-foreground hover:bg-accent/50 hover:text-foreground"
          >
            <PanelLeftOpen className="size-4" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="right">Expand rail ({expandHint})</TooltipContent>
      </Tooltip>
      {liveColor && (
        <span
          className="flex items-center gap-1 py-1 font-mono text-[10px] text-muted-foreground/70"
          title={`${liveN} running session${liveN === 1 ? "" : "s"}`}
        >
          <span className={cn("size-2 rounded-full", liveColor)} />
          {liveN}
        </span>
      )}
      {attentionCount > 0 && (
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label="Expand the rail to see attention items"
              onClick={onExpand}
              className="mt-1 rounded-md border border-amber-500/50 bg-amber-500/10 px-1.5 py-0.5 font-mono text-[10px] text-amber-500 hover:bg-amber-500/20"
            >
              {attentionCount} ⚑
            </button>
          </TooltipTrigger>
          <TooltipContent side="right">
            {attentionCount} attention item{attentionCount === 1 ? "" : "s"} (failing PRs,
            imminent meeting) — expand to see
          </TooltipContent>
        </Tooltip>
      )}
      <div className="my-1.5 h-px w-6 shrink-0 bg-border" />
      <div className="flex min-h-0 flex-1 flex-col items-center gap-1 overflow-y-auto">
        {repos.map((repo, i) => {
          const solo = isSoloRepo(repo);
          return (
            <div key={repo.key} className="flex flex-col items-center gap-1">
              {i > 0 && <div className="my-0.5 h-px w-6 bg-border" />}
              {(solo ? [repo.folders[0]] : repo.folders).map((f) => folderIcon(repo, f, solo))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** The board-wide agent tally pinned atop the rail: total + non-zero status
 * buckets + a ❄ compact count, with the Agentboard settings (compact
 * threshold) behind the trailing ⚙. Quiet when the board is at rest. */
export function RollupChip({ state, now }: { state: StatePayload; now: number }) {
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
          {r.busy > 0 && <DotCount status="busy" n={r.busy} />}
          {r.waiting > 0 && <DotCount status="waiting" n={r.waiting} />}
          {r.error > 0 && <DotCount status="error" n={r.error} />}
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
            aria-label="Agentboard settings"
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

export function RepoGroup({
  repo,
  now,
  compactPct,
  prs,
  selectedSessionId,
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
  onOpenDiff,
}: {
  repo: RepoData;
  now: number;
  compactPct: number;
  prs: PrItem[];
  selectedSessionId: string | null;
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
  /** Opens the folder's diff pane in its focused window. */
  onOpenDiff: (dir: string) => void;
}) {
  const solo = isSoloRepo(repo);

  const sessionRow = (folder: FolderData, s: SessionData) => (
    <SessionRow
      key={s.id}
      session={s}
      folderDir={folder.dir}
      now={now}
      compactPct={compactPct}
      title={titles[s.id]}
      active={selectedSessionId === s.id}
      renaming={renaming === s.id}
      overlay={overlays[s.id]}
      wins={wins}
      actions={actions}
      onSelect={() => onSelect(folder.dir, s.id)}
      onRenameCommit={(name) => onRenameCommit(s.id, name)}
    />
  );

  // Sessions render grouped by the window (pane group) they belong to: each
  // window that holds panes gets a thin label, then its sessions; sessions in
  // no window ("loose" shells) list on their own below. Grouping is purely
  // visual — the ⊟/click mechanics that move panes in and out of windows are
  // unchanged.
  const sessionRows = (folder: FolderData) => {
    if (folder.sessions.length === 0) {
      return (
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
      );
    }
    const folderWins = (wins?.windows ?? []).filter((w) => w.folderDir === folder.dir);
    const byId = new Map(folder.sessions.map((s) => [s.id, s] as const));
    const grouped = new Set(folderWins.flatMap((w) => w.panes));
    const loose = folder.sessions.filter((s) => !grouped.has(s.id));
    const groups = folderWins
      .map((w) => ({
        win: w,
        sessions: w.panes
          .map((id) => byId.get(id))
          .filter((s): s is SessionData => s !== undefined),
      }))
      .filter((g) => g.sessions.length > 0);
    return (
      <>
        {groups.map(({ win, sessions }) => (
          <div key={win.id}>
            <WindowLabel
              win={win}
              folderWins={folderWins}
              count={sessions.length}
              onFocus={() => actions.focusWindow(win.id)}
            />
            {sessions.map((s) => sessionRow(folder, s))}
          </div>
        ))}
        {loose.map((s) => sessionRow(folder, s))}
      </>
    );
  };

  // Solo repo: collapse repo + folder into one header (repo · branch).
  if (solo) {
    const folder = repo.folders[0];
    const isCollapsed = collapsed[repo.key];
    return (
      <div className="border-b">
        <FolderHeader
          scope="repo"
          title={repo.name}
          folder={folder}
          needs={repo.needs}
          pr={prForFolder(prs, repo.originUrl, folder.branch)}
          collapsed={isCollapsed}
          now={now}
          active={activeFolderDir === folder.dir}
          onToggle={() => {
            onToggle(repo.key);
            onSelectFolder(folder.dir);
          }}
          onNewSession={() => onNewSession(folder.dir)}
          onRemoveRepo={() => onRemoveRepo([folder.dir], repo.name)}
          onOpenDiff={() => onOpenDiff(folder.dir)}
        />
        {/* The note is a folder label — visible under the header even when the
            folder is collapsed (renders nothing when unset). */}
        <PurposeRow folder={folder} />
        {!isCollapsed && <div className="pb-2">{sessionRows(folder)}</div>}
      </div>
    );
  }

  // Multi-checkout repo: repo header, then each folder as a sub-header.
  const repoCollapsed = collapsed[repo.key];
  return (
    <div className="border-b">
      <div className="sticky top-0 z-10 flex w-full items-center gap-2 border-b border-l-2 border-border border-l-transparent bg-card px-3 py-2 hover:bg-accent/50">
        <button
          type="button"
          onClick={() => onToggle(repo.key)}
          className="flex min-w-0 flex-1 items-center gap-2"
        >
          <Chevron collapsed={repoCollapsed} />
          <FolderGit2 className="size-3.5 shrink-0 text-muted-foreground" />
          <span className="truncate text-sm font-semibold">{repo.name}</span>
          <span className="ml-auto flex items-center gap-2">
            {repoCollapsed && (
              <CollapsedLive sessions={repo.folders.flatMap((f) => f.sessions)} />
            )}
            {repo.needs > 0 && <NeedsBadge n={repo.needs} />}
          </span>
        </button>
        <RepoMenu
          onRemove={() =>
            onRemoveRepo(
              repo.folders.map((f) => f.dir),
              repo.name,
            )
          }
          dir={repo.folders[0].dir}
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
                folder={folder}
                needs={folder.needs}
                pr={prForFolder(prs, repo.originUrl, folder.branch)}
                collapsed={fCollapsed}
                now={now}
                active={activeFolderDir === folder.dir}
                onToggle={() => {
                  onToggle(key);
                  onSelectFolder(folder.dir);
                }}
                onNewSession={() => onNewSession(folder.dir)}
                onRemoveRepo={() => onRemoveRepo([folder.dir], folder.name)}
                onOpenDiff={() => onOpenDiff(folder.dir)}
              />
              {/* Note is a folder label — shown under the header even when the
                  folder is collapsed (renders nothing when unset). */}
              <PurposeRow folder={folder} />
              {!fCollapsed && <div className="pb-1">{sessionRows(folder)}</div>}
            </div>
          );
        })}
    </div>
  );
}

function FolderHeader({
  scope,
  title,
  folder,
  needs,
  pr,
  collapsed,
  active,
  now,
  onToggle,
  onNewSession,
  onRemoveRepo,
  onOpenDiff,
}: {
  scope: "repo" | "folder";
  /** repo.name at repo scope, folder.name at folder scope. */
  title: string;
  /** The checkout this header describes: dir, branch, worktree + diff facts. */
  folder: FolderData;
  needs: number;
  /** The open PR for this folder's branch, when the store knows of one. */
  pr?: PrItem;
  collapsed: boolean;
  /** Whether this folder is the one currently shown in the main pane area. */
  active: boolean;
  now: number;
  onToggle: () => void;
  onNewSession: () => void;
  onRemoveRepo?: () => void;
  /** Opens the folder's diff pane in its focused window. */
  onOpenDiff: () => void;
}) {
  const scopePrefix = pathScope(folder.dir);
  const progress = folder.metadata?.progress;
  return (
    // Two lines: name (line 1) with the git facts — branch, worktree, diff,
    // PR — grouped underneath (line 2), so a long branch never squeezes the
    // name and all git info reads as one cluster. The transparent border-l-2
    // is always present so the active violet edge never shifts content.
    <div
      className={cn(
        "border-b border-l-2 border-border border-l-transparent bg-card pr-2 hover:bg-accent/50",
        scope === "repo" ? "sticky top-0 z-10 pl-3" : "pl-6",
        active && "border-l-violet-500 bg-accent/60",
      )}
    >
      <div className="flex items-center gap-2 pt-1.5">
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
          {scopePrefix && (
            <span className="shrink-0 font-mono text-sm text-muted-foreground/60">
              {scopePrefix}
            </span>
          )}
          <span
            className={cn(
              "min-w-0 truncate",
              scope === "repo"
                ? "text-sm font-semibold"
                : "text-sm font-medium text-muted-foreground",
            )}
          >
            {title}
          </span>
        </button>
        {collapsed && <CollapsedLive sessions={folder.sessions} />}
        {needs > 0 && <NeedsBadge n={needs} />}
        <IconBtn title="New session (⌘D)" onClick={onNewSession} className="hover:text-violet-500">
          <Plus className="size-3.5" />
        </IconBtn>
        {onRemoveRepo && (
          <RepoMenu path={folder.dir} onRemove={onRemoveRepo} dir={folder.dir} folder={folder} />
        )}
      </div>
      {/* ml-11 lines the git row up under the name (chevron + icon + gaps). */}
      <div className="ml-11 flex items-center gap-1.5 pb-1.5">
        <span
          className="min-w-0 truncate font-mono text-[11px] text-muted-foreground"
          onClick={onToggle}
        >
          ⎇ {folder.branch}
        </span>
        <AheadBehind stats={folder} />
        {folder.isWorktree && <WorktreeBadge />}
        <DiffButton stats={folder} onOpen={onOpenDiff} />
        {pr && <PrChip pr={pr} />}
        {typeof progress?.percent === "number" && (
          <span
            title={progress.label ?? "agent-reported progress"}
            className="shrink-0 rounded-md border border-violet-500/40 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500"
          >
            {Math.round(progress.percent)}%{progress.label ? ` ${progress.label}` : ""}
          </span>
        )}
      </div>
      {/* The agent's own status line (ab_set_status), when one was pushed. */}
      <div className="ml-11 empty:hidden [&:not(:empty)]:pb-1.5">
        <AgentStatusLine metadata={folder.metadata} now={now} />
      </div>
    </div>
  );
}

/** Kebab menu on a repo/folder header: shows the full folder path (when
 * given), "Set/Edit note…" (when a `folder` is given — the note that shows
 * under the folder in the rail), "Create issue…" (shells `gh issue create` in
 * `dir`), and "Remove from rail". */
function RepoMenu({
  path,
  onRemove,
  dir,
  folder,
}: {
  path?: string;
  onRemove: () => void;
  dir: string;
  /** When set, the menu offers note editing for this checkout. */
  folder?: FolderData;
}) {
  const [issueOpen, setIssueOpen] = useState(false);
  const [issueTitle, setIssueTitle] = useState("");
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteText, setNoteText] = useState("");
  const purpose = folder?.purpose?.trim() ?? "";

  async function createIssue() {
    const title = issueTitle.trim();
    if (!title) return;
    setIssueOpen(false);
    setIssueTitle("");
    try {
      const url = await abCreateIssue(dir, title);
      toast.success("Issue created", {
        action: { label: "Open", onClick: () => void openExternalUrl(url) },
      });
    } catch (e) {
      toast.error(String(e));
    }
  }

  async function saveNote() {
    setNoteOpen(false);
    const trimmed = noteText.trim();
    if (trimmed === purpose) return;
    await abInvoke("ab_set_folder_purpose", { dir, text: trimmed || null });
  }

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="icon-xs"
            title="Repo actions"
            className="text-muted-foreground"
          >
            <MoreVertical className="size-3.5" />
          </Button>
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
          {folder && (
            <DropdownMenuItem
              onSelect={() => {
                setNoteText(purpose);
                setNoteOpen(true);
              }}
              className="whitespace-nowrap"
            >
              <StickyNote className="size-3.5" /> {purpose ? "Edit note…" : "Set note…"}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onSelect={() => setIssueOpen(true)} className="whitespace-nowrap">
            <CircleDot className="size-3.5" /> Create issue…
          </DropdownMenuItem>
          <DropdownMenuItem
            variant="destructive"
            onSelect={onRemove}
            className="whitespace-nowrap"
          >
            <Trash2 className="size-3.5" /> Remove from rail
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={noteOpen} onOpenChange={setNoteOpen}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>{purpose ? "Edit note" : "Set note"}</DialogTitle>
          </DialogHeader>
          <Input
            autoFocus
            value={noteText}
            onChange={(e) => setNoteText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void saveNote();
              if (e.key === "Escape") setNoteOpen(false);
            }}
            placeholder="what are you working toward here? (blank clears)"
          />
        </DialogContent>
      </Dialog>
      <Dialog open={issueOpen} onOpenChange={setIssueOpen}>
        <DialogContent showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>New issue</DialogTitle>
          </DialogHeader>
          <Input
            autoFocus
            value={issueTitle}
            onChange={(e) => setIssueTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void createIssue();
            }}
            placeholder="Issue title…"
          />
        </DialogContent>
      </Dialog>
    </>
  );
}

/** Thin header above a window's panes in the rail: a color chip + window name
 * + pane count, clicking focuses that window in the pane area. Deliberately
 * small — grouping should add structure to the rail, not bulk. */
function WindowLabel({
  win,
  folderWins,
  count,
  onFocus,
}: {
  win: AgWindow;
  folderWins: AgWindow[];
  count: number;
  onFocus: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onFocus}
      title={`window “${win.name}” — click to focus`}
      className="flex w-full items-center gap-1.5 pt-1 pr-3 pb-0.5 pl-9 text-left"
    >
      <span className={cn("size-2 shrink-0 rounded-[3px]", windowColor(folderWins, win.id))} />
      <span className="truncate font-mono text-[10px] uppercase tracking-wide text-muted-foreground/70">
        {win.name}
      </span>
      <span className="font-mono text-[9.5px] text-muted-foreground/50">{count}⊞</span>
    </button>
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
  const needs = sessionCatchesEye(eff);
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
      aria-current={active || undefined}
      title={eff.purpose ? `✦ ${eff.purpose}` : undefined}
      onClick={onSelect}
      onDoubleClick={() => actions.renameStart(session.id)}
      onKeyDown={(e) => e.key === "Enter" && onSelect()}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      className={cn(
        "relative ml-1.5 flex cursor-pointer items-center gap-2.5 border-l-2 border-transparent py-1.5 pr-3 pl-9",
        hovered && "bg-accent",
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
          {/* Window membership is shown by the WindowLabel grouping above, so
              no per-row window chip here. */}
          {/* Meta cluster stays in the flow permanently — the lifecycle
              controls overlay it (absolute, opaque accent) instead of
              swapping it out, so hovering never reflows the row. */}
          <span className="ml-auto flex min-w-0 shrink items-center gap-2">
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
          {needs && (
            <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />
          )}
          {(active || hovered) && (
            <span className="absolute inset-y-0 right-2 z-10 flex items-center gap-1 bg-accent pl-1.5">
              <RowControls session={eff} folderDir={folderDir} grouped={!!grouped} actions={actions} />
            </span>
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
    className = "hover:text-foreground",
  ) => (
    <IconBtn title={title} onClick={onClick} className={className}>
      {label}
    </IconBtn>
  );

  return (
    <>
      {!session.live &&
        btn("▶", "start shell", () => actions.start(folderDir, session), "hover:text-green-500")}
      {(!session.live || !agent) &&
        btn("✦", "start Claude here", () => actions.startClaude(folderDir, session), "text-violet-500 hover:text-violet-400")}
      {session.live && agent && (
        <>
          {btn("■", "stop Claude (shell survives)", () => actions.stopClaude(session), "hover:text-red-500")}
          {atPrompt && btn("⤿", "compact context (/compact)", () => actions.compactClaude(session), "hover:text-sky-500")}
          {btn("↻", "start over — fresh Claude session", () => actions.restartClaude(folderDir, session), "hover:text-orange-500")}
        </>
      )}
      {grouped &&
        btn("⊟", "ungroup — remove pane from its window", () => actions.ungroup(session.id), "hover:text-sky-500")}
      {btn("✎", "rename", () => actions.renameStart(session.id))}
      {btn("✕", "close session", () => actions.close(session.id), "hover:text-red-500")}
    </>
  );
}
