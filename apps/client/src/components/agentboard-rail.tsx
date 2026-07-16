import { useState } from "react";
import {
  Folder,
  FolderGit2,
  FolderPlus,
  FolderX,
  MoreVertical,
  PanelLeftOpen,
  Plus,
  Trash2,
} from "lucide-react";
import {
  AgentStatusLine,
  AheadBehind,
  CacheBadge,
  Chevron,
  DiffButton,
  FilesButton,
  Dot,
  DotCount,
  GhostBadge,
  Glyph,
  IconBtn,
  NeedsBadge,
  PortDriftBadge,
  PrChip,
  PurposeRow,
  RepoMenu,
  WorktreeBadge,
} from "@/components/agentboard-bits";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { InlineNewSlot, PendingSlotRow, type NewSlotRepo, type PendingSlot } from "@/components/inline-new-slot";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Slider } from "@/components/ui/slider";
import { cn } from "@/lib/utils";
import {
  abInvoke,
  agentRollup,
  claudeTitleName,
  fmtElapsed,
  fmtWaitingAge,
  folderPortDrift,
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
  type ClaudeLaunchOptions,
  type FolderData,
  type Overlay,
  type RepoData,
  type SessionActions,
  type SessionData,
  type StatePayload,
  type WindowsPayload,
} from "@/lib/agentboard";
import type { PrItem } from "@/lib/data";

/** Ambient status color for a set of sessions hidden behind a collapse:
 * red if one errored, blue if one is waiting on you, cyan while an agent is
 * busy, else a calm emerald for "live but idle". Null when nothing is live. */
function collapsedLiveColor(sessions: SessionData[]): string | null {
  const live = sessions.filter((s) => s.live);
  if (live.length === 0) return null;
  if (live.some((s) => s.agentState?.status === "error")) return "bg-red-500";
  if (live.some((s) => s.agentState?.status === "waiting")) return "bg-blue-500";
  if (live.some((s) => s.agentState?.status === "busy")) return "bg-cyan-500";
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
          {r.expiring > 0 && (
            <span className="text-amber-500" title="warm prompt caches about to expire — nudge them">
              ◔{r.expiring}
            </span>
          )}
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
  onNewSlot,
  onRemoveRepo,
  onDeleteWorktree,
  onRenameCommit,
  onOpenDiff,
  onOpenFiles,
  quietDirs,
  quietRevealed,
  onToggleQuiet,
  slotFormOpen,
  onCancelSlotForm,
  onSubmitSlotForm,
  pendingSlots,
  onRetryPendingSlot,
  onDismissPendingSlot,
  onCreateTemplateRetry,
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
  /** Toggles the inline new-slot form open/closed for a slot-convention repo
   * (worktree hub) — never a blocking modal, see InlineNewSlot. */
  onNewSlot: (repo: NewSlotRepo) => void;
  onRemoveRepo: (dirs: string[], label: string) => void;
  /** Delete a worktree slot from disk (guarded `slot_remove`). */
  onDeleteWorktree: (dir: string, label: string) => void;
  onRenameCommit: (sessionId: string, name: string) => void;
  /** Opens the folder's diff pane in its focused window. */
  onOpenDiff: (dir: string) => void;
  /** Opens the folder's files pane in its focused window. */
  onOpenFiles: (dir: string) => void;
  /** Dirs the hide-inactive filter tucks behind a "N quiet" stub (empty/
   * undefined when the filter is off). Quiet folders demote to the stub
   * instead of vanishing — nothing ever silently disappears from the rail. */
  quietDirs?: Set<string>;
  /** Whether this repo's quiet folders are temporarily shown. */
  quietRevealed?: boolean;
  onToggleQuiet?: () => void;
  /** Whether this repo's inline new-slot form is open. */
  slotFormOpen: boolean;
  onCancelSlotForm: () => void;
  onSubmitSlotForm: (input: {
    goal: string;
    branch: string;
    base: string;
    options: ClaudeLaunchOptions;
  }) => void;
  /** This repo's in-flight `slot_create` calls — see PendingSlot. */
  pendingSlots: PendingSlot[];
  onRetryPendingSlot: (id: string) => void;
  onDismissPendingSlot: (id: string) => void;
  onCreateTemplateRetry: (id: string) => void;
}) {
  const solo = isSoloRepo(repo);
  const quiet = quietDirs ?? new Set<string>();
  const showQuiet = quietRevealed ?? false;

  const pendingRows = pendingSlots.map((p) => (
    <PendingSlotRow
      key={p.id}
      pending={p}
      now={now}
      onRetry={onRetryPendingSlot}
      onDismiss={onDismissPendingSlot}
      onCreateTemplate={onCreateTemplateRetry}
    />
  ));

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

  // Sessions render grouped by the window (pane group) they belong to: a
  // window holding multiple panes gets a vertical color spine running beside
  // its rows (no text label — window names carry no signal in the rail);
  // sessions in no window ("loose" shells) list on their own below. Grouping
  // is purely visual — the ⊟/click mechanics that move panes in and out of
  // windows are unchanged.
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
          <div key={win.id} className="relative">
            {sessions.length > 1 && (
              <WindowSpine
                win={win}
                folderWins={folderWins}
                count={sessions.length}
                onFocus={() => actions.focusWindow(win.id)}
              />
            )}
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
    if (quiet.has(folder.dir) && !showQuiet) {
      return <QuietRepoStub name={repo.name} count={1} onToggle={onToggleQuiet} />;
    }
    return (
      <div className="border-b" data-focus-kind="repo" data-focus-id={repo.key}>
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
          onNewSlot={() => onNewSlot({ name: repo.name, dir: folder.dir, key: repo.key })}
          onRemoveRepo={() => onRemoveRepo([folder.dir], repo.name)}
          onDeleteWorktree={
            folder.isWorktree ? () => onDeleteWorktree(folder.dir, repo.name) : undefined
          }
          onOpenDiff={() => onOpenDiff(folder.dir)}
          onOpenFiles={() => onOpenFiles(folder.dir)}
        />
        {/* The note is a folder label — visible under the header even when the
            folder is collapsed (renders nothing when unset). */}
        <PurposeRow folder={folder} />
        {slotFormOpen && (
          <InlineNewSlot
            repo={{ name: repo.name, dir: folder.dir, key: repo.key }}
            onCancel={onCancelSlotForm}
            onSubmit={onSubmitSlotForm}
          />
        )}
        {pendingRows}
        {!isCollapsed && <div className="pb-2">{sessionRows(folder)}</div>}
        {quiet.size > 0 && showQuiet && (
          <QuietToggleRow count={quiet.size} revealed onToggle={onToggleQuiet} />
        )}
      </div>
    );
  }

  // Multi-checkout repo: repo header, then each folder as a sub-header. Quiet
  // folders (hide-inactive filter) tuck behind a stub toggle row; a repo with
  // *only* quiet folders shrinks to a single dim stub line.
  const repoCollapsed = collapsed[repo.key];
  const shownFolders = showQuiet ? repo.folders : repo.folders.filter((f) => !quiet.has(f.dir));
  if (shownFolders.length === 0) {
    return <QuietRepoStub name={repo.name} count={quiet.size} onToggle={onToggleQuiet} />;
  }
  // One of this repo's checkouts is the focused folder — bubble the violet
  // active edge up to the repo header too, so a collapsed (or just
  // easy-to-miss) repo row still shows it holds the folder you're looking
  // at (folder-rail rule: focus never stops at the child level).
  const repoActive = repo.folders.some((f) => f.dir === activeFolderDir);
  return (
    <div className="border-b" data-focus-kind="repo" data-focus-id={repo.key}>
      <div
        className={cn(
          "sticky top-0 z-10 flex w-full items-center gap-2 border-b border-l-2 border-border border-l-transparent bg-card px-3 py-2 hover:bg-accent/50",
          repoActive && "border-l-violet-500 bg-accent/60",
        )}
      >
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
        <IconBtn
          title="New slot — goal, branch, base"
          onClick={() => onNewSlot({ name: repo.name, dir: repo.folders[0].dir, key: repo.key })}
          className="hover:text-violet-500"
        >
          <FolderPlus className="size-3.5" />
        </IconBtn>
        <RepoMenu
          onRemove={() =>
            onRemoveRepo(
              repo.folders.map((f) => f.dir),
              repo.name,
            )
          }
          dir={repo.folders[0].dir}
          onNewSlot={() => onNewSlot({ name: repo.name, dir: repo.folders[0].dir, key: repo.key })}
        />
      </div>
      {slotFormOpen && (
        <InlineNewSlot
          repo={{ name: repo.name, dir: repo.folders[0].dir, key: repo.key }}
          onCancel={onCancelSlotForm}
          onSubmit={onSubmitSlotForm}
        />
      )}
      {pendingRows}
      {!repoCollapsed &&
        shownFolders.map((folder) => {
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
                onDeleteWorktree={
                  folder.isWorktree
                    ? () => onDeleteWorktree(folder.dir, folder.name)
                    : undefined
                }
                onOpenDiff={() => onOpenDiff(folder.dir)}
                onOpenFiles={() => onOpenFiles(folder.dir)}
              />
              {/* Note is a folder label — shown under the header even when the
                  folder is collapsed (renders nothing when unset). */}
              <PurposeRow folder={folder} />
              {!fCollapsed && <div className="pb-1">{sessionRows(folder)}</div>}
            </div>
          );
        })}
      {!repoCollapsed && quiet.size > 0 && (
        <QuietToggleRow count={quiet.size} revealed={showQuiet} onToggle={onToggleQuiet} />
      )}
    </div>
  );
}

/** A repo whose checkouts are all quiet (hide-inactive filter), demoted to
 * one dim row instead of removed — the repo stays findable, just out of the
 * way. Clicking restores the full group until toggled back. */
function QuietRepoStub({
  name,
  count,
  onToggle,
}: {
  name: string;
  count: number;
  onToggle?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      title="Nothing going on here right now — click to show"
      className="flex w-full items-center gap-2 border-b bg-card px-3 py-1.5 text-left text-muted-foreground/60 hover:bg-accent/40 hover:text-muted-foreground"
    >
      <Chevron collapsed />
      <FolderGit2 className="size-3.5 shrink-0 opacity-60" />
      <span className="min-w-0 truncate text-sm">{name}</span>
      <span className="ml-auto shrink-0 font-mono text-[10px]">
        {count === 1 ? "quiet" : `${count} quiet`}
      </span>
    </button>
  );
}

/** The stub/toggle row under a repo's visible folders: "N quiet" when its
 * quiet checkouts are tucked away, "hide N quiet" while they're shown. */
function QuietToggleRow({
  count,
  revealed,
  onToggle,
}: {
  count: number;
  revealed: boolean;
  onToggle?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      className="flex w-full items-center gap-1.5 py-1 pr-3 pl-6 text-left font-mono text-[10.5px] text-muted-foreground/50 hover:text-muted-foreground"
    >
      <Chevron collapsed={!revealed} />
      {revealed ? `hide ${count} quiet` : `${count} quiet`}
    </button>
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
  onNewSlot,
  onRemoveRepo,
  onDeleteWorktree,
  onOpenDiff,
  onOpenFiles,
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
  /** Opens the new-slot modal — set only on a solo slot-convention repo's
   * collapsed repo+folder header (the multi-checkout repo tier renders its
   * own button). */
  onNewSlot?: () => void;
  onRemoveRepo?: () => void;
  /** Deletes this worktree slot from disk (guarded, `slot_remove`) — set
   * only on worktree checkouts, where untracking makes no sense (they are
   * auto-discovered from the primary and would reappear next poll). */
  onDeleteWorktree?: () => void;
  /** Opens the folder's diff pane in its focused window. */
  onOpenDiff: () => void;
  /** Opens the folder's files pane in its focused window. */
  onOpenFiles: () => void;
}) {
  const scopePrefix = pathScope(folder.dir);
  const progress = folder.metadata?.progress;
  // Ghost checkout: the tracked directory is gone. Dim the whole band and
  // swap the git-facts line (branch/diff are meaningless) for an inline
  // Untrack — a dead folder's one useful action, surfaced not buried.
  const missing = folder.dirMissing;
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
          className={cn(
            "flex min-w-0 flex-1 items-center gap-2",
            // Ghost: dim the identity cluster so it reads as inert. The action
            // buttons (Untrack, kebab) sit outside this and stay full-strength.
            missing && "opacity-60",
          )}
        >
          <Chevron collapsed={collapsed} />
          {missing ? (
            <FolderX className="size-3.5 shrink-0 text-muted-foreground/70" />
          ) : scope === "repo" ? (
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
              missing && "line-through decoration-muted-foreground/40",
            )}
          >
            {title}
          </span>
          {missing && <GhostBadge />}
        </button>
        {collapsed && !missing && <CollapsedLive sessions={folder.sessions} />}
        {needs > 0 && <NeedsBadge n={needs} />}
        {/* No "New session"/"New slot" on a ghost — the directory is gone. */}
        {!missing && (
          <IconBtn title="New session (⌘D)" onClick={onNewSession} className="hover:text-violet-500">
            <Plus className="size-3.5" />
          </IconBtn>
        )}
        {!missing && onNewSlot && (
          <IconBtn title="New slot — goal, branch, base" onClick={onNewSlot} className="hover:text-violet-500">
            <FolderPlus className="size-3.5" />
          </IconBtn>
        )}
        {onRemoveRepo && (
          <RepoMenu
            path={folder.dir}
            onRemove={onRemoveRepo}
            dir={folder.dir}
            folder={folder}
            isWorktree={folder.isWorktree}
            onNewSlot={!missing ? onNewSlot : undefined}
            onDeleteWorktree={!missing ? onDeleteWorktree : undefined}
          />
        )}
      </div>
      {/* ml-11 lines the git row up under the name (chevron + icon + gaps). */}
      {missing ? (
        <div className="ml-11 flex items-center gap-2 pb-1.5">
          <span className="min-w-0 truncate text-[11px] text-muted-foreground/70 italic">
            directory missing — moved or deleted
          </span>
          {onRemoveRepo && (
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                onRemoveRepo();
              }}
              title="Untrack this checkout — remove it from the rail"
              className="flex h-5 shrink-0 items-center gap-1 rounded-md border border-border/70 px-1.5 font-mono text-[10.5px] text-muted-foreground transition-colors hover:border-red-500/50 hover:bg-red-500/10 hover:text-red-600 dark:hover:text-red-400"
            >
              <Trash2 className="size-3" /> Untrack
            </button>
          )}
        </div>
      ) : (
        <div className="ml-11 flex items-center gap-1.5 pb-1.5">
          <span
            className="min-w-0 truncate font-mono text-[11px] text-muted-foreground"
            onClick={onToggle}
          >
            ⎇ {folder.branch}
          </span>
          <AheadBehind stats={folder} />
          {folder.isWorktree && <WorktreeBadge />}
          {folder.hasPortDrift && <PortDriftBadge drift={folderPortDrift(folder)} />}
          <DiffButton stats={folder} onOpen={onOpenDiff} />
          <FilesButton onOpen={onOpenFiles} />
          {pr && <PrChip pr={pr} stats={folder} />}
          {typeof progress?.percent === "number" && (
            <span
              title={progress.label ?? "agent-reported progress"}
              className="shrink-0 rounded-md border border-violet-500/40 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500"
            >
              {Math.round(progress.percent)}%{progress.label ? ` ${progress.label}` : ""}
            </span>
          )}
        </div>
      )}
      {/* The agent's own status line (ab_set_status), when one was pushed. */}
      <div className="ml-11 empty:hidden [&:not(:empty)]:pb-1.5">
        <AgentStatusLine metadata={folder.metadata} now={now} />
      </div>
    </div>
  );
}

/** Vertical color spine beside a multi-pane window's rows in the rail: the
 * window's group color as a thin bar bracketing its sessions, clicking
 * focuses that window in the pane area. Replaces the old text label — window
 * names carry no signal in the rail; the color + tooltip is enough. */
function WindowSpine({
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
      title={`window “${win.name}” — ${count} panes, click to focus`}
      aria-label={`Focus window ${win.name}`}
      className="absolute inset-y-1 left-4 z-10 flex w-2 justify-center"
    >
      <span className={cn("h-full w-[3px] rounded-full", windowColor(folderWins, win.id))} />
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
  // Prefer the live Claude terminal title (`✳ <title>`) only while the shell is
  // actually running — a stopped PTY's last title lingers in the caller's
  // `titles` map (never cleared) and would otherwise label a dead shell as a
  // running Claude.
  const label = (eff.live ? claudeTitleName(title) : null) ?? sessionLabel(eff);
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
        hovered && !needs && "bg-accent",
        active && !needs && "border-l-violet-500 bg-accent",
        // Needs-you wins over hover/active for both the edge and the fill —
        // a thin 2px border alone was too easy to miss scanning the rail, so
        // the whole row washes amber, not just its left pixel.
        needs && "border-l-amber-500 bg-amber-500/10",
        needs && hovered && "bg-amber-500/15",
      )}
    >
      <Glyph agent={agent} />
      <Dot session={eff} />
      {needs && <span className="size-1.5 shrink-0 rounded-full bg-amber-500" />}
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
            {eff.live && <PortDriftBadge drift={eff.portDrift ?? []} />}
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
            {(() => {
              const age = fmtWaitingAge(eff.needsSinceMs, now);
              return age ? (
                <span
                  className="shrink-0 font-mono text-[10.5px] text-amber-500/80"
                  title="how long this has been needing you"
                >
                  {age}
                </span>
              ) : null;
            })()}
            <span className="min-w-0 truncate text-[11px] text-muted-foreground">
              {sessionStatusText(eff)}
            </span>
          </span>
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

/** Hover-reveal lifecycle controls for a session row: ✕ close stays inline
 * (the one action common to every row), everything else — which varies by
 * state (not started → ▶ shell / ✦ Claude; live shell → ✦ Claude; live agent
 * → ■ stop / ⤿ compact / ↻ restart; grouped → ⊟ ungroup; plus ✎ rename) —
 * lives behind a "···" menu instead of crowding the row. */
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

  const items: {
    glyph: string;
    label: string;
    onSelect: () => void;
    className?: string;
  }[] = [];
  if (!session.live) {
    items.push({
      glyph: "▶",
      label: "Start shell",
      onSelect: () => actions.start(folderDir, session),
      className: "text-green-500",
    });
  }
  if (!session.live || !agent) {
    items.push({
      glyph: "✦",
      label: "Start Claude here",
      onSelect: () => actions.startClaude(folderDir, session),
      className: "text-violet-500",
    });
  }
  if (session.live && agent) {
    items.push({
      glyph: "■",
      label: "Stop Claude (shell survives)",
      onSelect: () => actions.stopClaude(session),
    });
    if (atPrompt) {
      items.push({
        glyph: "⤿",
        label: "Compact context (/compact)",
        onSelect: () => actions.compactClaude(session),
      });
    }
    items.push({
      glyph: "↻",
      label: "Start over — fresh Claude session",
      onSelect: () => actions.restartClaude(folderDir, session),
    });
  }
  if (grouped) {
    items.push({
      glyph: "⊟",
      label: "Ungroup — remove pane from its window",
      onSelect: () => actions.ungroup(session.id),
    });
  }
  items.push({ glyph: "✎", label: "Rename", onSelect: () => actions.renameStart(session.id) });

  return (
    <>
      <IconBtn
        title="close session"
        onClick={() => actions.close(session.id)}
        className="hover:text-red-500"
      >
        ✕
      </IconBtn>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="icon-xs"
            title="More actions"
            className="text-muted-foreground"
            onClick={(e) => e.stopPropagation()}
          >
            <MoreVertical className="size-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-auto min-w-48">
          {items.map((item) => (
            <DropdownMenuItem
              key={item.label}
              onSelect={item.onSelect}
              className="whitespace-nowrap"
            >
              <span className={cn("w-4 text-center font-mono text-xs", item.className)}>
                {item.glyph}
              </span>
              {item.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}
