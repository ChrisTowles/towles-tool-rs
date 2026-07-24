import { useState } from "react";
import { FolderGit2, FolderPlus, GitPullRequest, Plus, Trash2, X } from "lucide-react";
import {
  AgentStatusLine,
  AheadBehind,
  ComparedBaseBadge,
  DeletingBadge,
  DiffButton,
  Dot,
  FilesButton,
  FolderLandedBadge,
  fmtMins,
  GhostBadge,
  IconBtn,
  IssueChip,
  PortDriftBadge,
  PreviewButton,
  PrChip,
  RepoMenu,
  SafeToDeleteBadge,
  BranchLabel,
} from "@/components/agentboard-bits";
import { DevServersButton } from "@/components/dev-servers";
import { PaneChrome, PaneLens } from "@/components/pane-chrome";
import type { NewTaskRepo } from "@/components/inline-new-task";
import {
  branchRedundant,
  comparedBaseLabel,
  fmtElapsed,
  fmtWaitingAge,
  folderActionableItems,
  folderPortDrift,
  folderSafeToDelete,
  humanizeFolderName,
  isAgent,
  isCacheExpiring,
  isCold,
  modelContextLabel,
  pathScope,
  type ActionableItem,
  type ActionableKind,
  type FolderData,
  type RepoData,
  type SessionActions,
  type SessionData,
} from "@/lib/agentboard";
import type { PrItem, TaskItem } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";
import { shortcutHint } from "@/lib/shortcuts";
import { cn } from "@/lib/utils";

/** The working-context band atop the main pane: *where am I working*. Leads
 * with the focused checkout name — large, first, the anchor of the whole
 * screen — with the repo and branch (plus git facts: worktree badge, diff
 * button, PR chip) on a quieter line below it. One glance answers which
 * checkout the terminals below belong to; *what you set out to do there* is
 * the Board task's job. The trailing action cluster mirrors the rail's options
 * for this checkout — new session, new task, and the shared "···" RepoMenu —
 * so every repo-rail option stays reachable atop the panes even when the rail
 * is collapsed or the folder's row is scrolled out of view. */
export function WorkingContext({
  repo,
  folder,
  pr,
  task,
  now,
  deleting,
  actions,
  onOpenDiff,
  onOpenFiles,
  onOpenPreview,
  onNewSession,
  onNewTask,
  onRemoveRepo,
  onDeleteWorktree,
}: {
  repo: RepoData;
  folder: FolderData;
  pr?: PrItem;
  /** The board task bound to this checkout's worktree, when one exists —
   * source of the linked-issue chips, the "Attach issue…" target, and (for a
   * worktree) the human-authored title shown on line 1 — see the rail's
   * `FolderHeader` for the same derivation. */
  task?: TaskItem;
  /** Clock tick for `AgentStatusLine`'s relative "Nm ago" age. */
  now: number;
  /** This worktree's `task_delete` is in flight — mirrors the rail's
   * `DeletingBadge` gating. */
  deleting?: boolean;
  /** Session lifecycle dispatch — the dev-servers popover launches/focuses
   * through it. */
  actions: SessionActions;
  /** Opens the folder's diff pane in its focused window. */
  onOpenDiff: (dir: string) => void;
  /** Opens the folder's files pane in its focused window. */
  onOpenFiles: (dir: string) => void;
  /** Opens the folder's live-preview pane in its focused window. */
  onOpenPreview: (dir: string) => void;
  /** Starts a new session (shell) in this checkout. */
  onNewSession: (dir: string) => void;
  /** Toggles the inline new-task form open/closed for this repo (worktree
   * hub) — never a blocking modal, see InlineNewTask. The form itself still
   * renders in the rail under the repo's header, so this only opens it when
   * the rail is expanded; the caller is responsible for expanding a
   * collapsed rail first if it wants the form to be visible. */
  onNewTask: (repo: NewTaskRepo) => void;
  /** Untracks this checkout from the rail. */
  onRemoveRepo: (dirs: string[], label: string) => void;
  /** Deletes a worktree from disk (guarded `task_delete`). */
  onDeleteWorktree: (dir: string, label: string) => void;
}) {
  const scope = pathScope(folder.dir);
  // A task/worktree has a distinct checkout name; a lone clone shares the
  // repo's, so we don't repeat it on the line below.
  const repoDistinct = folder.name !== repo.name;
  // Same gating as the rail headers: no session/task actions on a ghost
  // checkout whose directory is gone.
  const missing = folder.dirMissing;
  // Same title/branch derivation as the rail's `FolderHeader` — a worktree
  // task's human-authored title takes line 1, and the branch stays visible
  // whenever it's carrying information the title doesn't already restate.
  const humanTitle = folder.isWorktree ? task?.text?.trim() : undefined;
  const displayTitle =
    humanTitle || (folder.isWorktree ? humanizeFolderName(folder.name) : folder.name);
  const showBranchLabel = Boolean(humanTitle) || !branchRedundant(folder.name, folder.branch);
  const progress = folder.metadata?.progress;
  const newTask = () => onNewTask({ name: repo.name, dir: repo.folders[0].dir, key: repo.key });
  return (
    <div className="flex items-start gap-3 border-b bg-card px-4 py-2.5">
      <FolderGit2 className="mt-0.5 size-5 shrink-0 text-violet-500" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {/* Line 1: the checkout — largest, first — with the rail's action
            cluster for this checkout pinned at the trailing edge. */}
        <div className="flex items-center gap-2">
          <span
            title={humanTitle ? folder.name : undefined}
            className="min-w-0 flex-1 truncate text-2xl font-semibold leading-tight"
          >
            {displayTitle}
          </span>
          {missing && <GhostBadge />}
          {/* Always mounted (dimmed sans launch.json) so the dev-servers
              feature is discoverable; the dense rail stays gated. */}
          {!missing && <DevServersButton folder={folder} actions={actions} />}
          {!missing && (
            <IconBtn
              title={`New session (${shortcutHint("ab-new-session")})`}
              onClick={() => onNewSession(folder.dir)}
              className="hover:text-violet-500"
            >
              <Plus className="size-3.5" />
            </IconBtn>
          )}
          {!missing && (
            <IconBtn
              title={`New task — goal, issues, branch (${shortcutHint("ab-new-task")})`}
              onClick={newTask}
              className="hover:text-violet-500"
            >
              <FolderPlus className="size-3.5" />
            </IconBtn>
          )}
          <RepoMenu
            path={folder.dir}
            dir={folder.dir}
            isWorktree={folder.isWorktree}
            quiet={folder.quiet}
            onNewTask={!missing ? newTask : undefined}
            onDeleteWorktree={
              !missing && folder.isWorktree
                ? () => onDeleteWorktree(folder.dir, folder.name)
                : undefined
            }
            onRemove={() => onRemoveRepo([folder.dir], folder.name)}
            taskId={!missing ? task?.id : undefined}
          />
        </div>
        {/* Line 2: repo · branch + git facts, quieter. */}
        <div className="flex min-w-0 flex-wrap items-center gap-1.5 text-sm text-muted-foreground">
          {scope && <span className="shrink-0 font-mono text-muted-foreground/60">{scope}</span>}
          {repoDistinct && <span className="shrink-0 font-medium">{repo.name}</span>}
          {showBranchLabel && <BranchLabel branch={folder.branch} isWorktree={folder.isWorktree} />}
          {deleting && <DeletingBadge />}
          <ComparedBaseBadge folder={folder} />
          <AheadBehind stats={folder} />
          {folder.hasPortDrift && <PortDriftBadge drift={folderPortDrift(folder)} />}
          <DiffButton stats={folder} onOpen={() => onOpenDiff(folder.dir)} />
          <FilesButton onOpen={() => onOpenFiles(folder.dir)} />
          {folder.hasLaunchConfig && <PreviewButton onOpen={() => onOpenPreview(folder.dir)} />}
          {pr && <PrChip pr={pr} stats={folder} />}
          {task &&
            task.issues.map((issue) => (
              <IssueChip key={`${issue.repo}#${issue.number}`} taskId={task.id} issue={issue} />
            ))}
          <FolderLandedBadge folder={folder} pr={pr} />
          {!missing && folder.isWorktree && folderSafeToDelete(folder, pr) && (
            <SafeToDeleteBadge
              base={comparedBaseLabel(folder)}
              landed={folder.landed}
              onDeleteWorktree={() => onDeleteWorktree(folder.dir, folder.name)}
            />
          )}
          {typeof progress?.percent === "number" && (
            <span
              title={progress.label ?? "agent-reported progress"}
              className="shrink-0 rounded-md border border-violet-500/40 bg-violet-500/10 px-1.5 font-mono text-[10.5px] text-violet-500"
            >
              {Math.round(progress.percent)}%{progress.label ? ` ${progress.label}` : ""}
            </span>
          )}
        </div>
        <AgentStatusLine metadata={folder.metadata} now={now} />
        {!missing && (
          <ActionableCallouts
            items={folderActionableItems(folder, pr)}
            folderDir={folder.dir}
            folderLabel={folder.name}
            onDeleteWorktree={onDeleteWorktree}
          />
        )}
      </div>
    </div>
  );
}

const ACTIONABLE_META: Record<
  ActionableKind,
  { heading: string; glyph: string; textClass: string; borderClass: string }
> = {
  "safe-to-delete": {
    heading: "Safe to delete",
    glyph: "✓",
    textClass: "text-emerald-600 dark:text-emerald-400",
    borderClass: "border-emerald-500/40",
  },
  "needs-you": {
    heading: "Needs you",
    glyph: "⚑",
    textClass: "text-amber-500",
    borderClass: "border-amber-500/40",
  },
  "port-drift": {
    heading: "Port drift",
    glyph: "⚡",
    textClass: "text-amber-500",
    borderClass: "border-amber-500/40",
  },
};

/** The working-context band's actionable section: a full-detail callout per
 * `ActionableItem` (usually at most one or two at once), replacing the rail
 * row's cramped badge with the room to say *why*. Only rendered for the
 * focused checkout — the rail keeps its own badges for scanning every other
 * folder at a glance. */
function ActionableCallouts({
  items,
  folderDir,
  folderLabel,
  onDeleteWorktree,
}: {
  items: ActionableItem[];
  folderDir: string;
  folderLabel: string;
  onDeleteWorktree: (dir: string, label: string) => void;
}) {
  if (items.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5 pt-1">
      {items.map((item) => {
        const meta = ACTIONABLE_META[item.kind];
        return (
          <div
            key={item.kind}
            className={cn(
              "flex items-center gap-2 rounded-md border border-l-2 bg-card px-2.5 py-1.5 text-xs",
              meta.borderClass,
            )}
          >
            <span className={cn("shrink-0 font-mono text-sm", meta.textClass)}>{meta.glyph}</span>
            <span className={cn("shrink-0 font-medium", meta.textClass)}>{meta.heading}</span>
            <span className="min-w-0 flex-1 truncate text-muted-foreground">{item.subtitle}</span>
            {item.pr && (
              <button
                type="button"
                onClick={() => void openExternalUrl(item.pr!.url)}
                title={`Open PR #${item.pr.number} on GitHub`}
                className="flex h-6 shrink-0 items-center gap-1 rounded-md border border-border/70 px-1.5 font-mono text-[10.5px] text-muted-foreground hover:bg-accent hover:text-foreground"
              >
                <GitPullRequest className="size-3" />#{item.pr.number}
              </button>
            )}
            {item.kind === "safe-to-delete" && (
              <button
                type="button"
                onClick={() => onDeleteWorktree(folderDir, folderLabel)}
                title="Delete this worktree — nothing here would be lost"
                className="flex h-6 shrink-0 items-center gap-1 rounded-md border border-emerald-500/50 bg-emerald-500/10 px-1.5 font-mono text-[10.5px] text-emerald-600 hover:bg-emerald-500/20 dark:text-emerald-400"
              >
                <Trash2 className="size-3" /> delete
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** Cache health for one pane, shown only while Claude is actually running in
 * it (a live agent for this session). Cache warmth only — no context percent —
 * so the pane chrome stays quiet: `⧗ 42m left` / `◔ 3m left` while warm
 * (amber once inside the warn window — nudge Claude before the cache lapses),
 * `❄ cache cold` once the prompt cache has expired. Nothing when no agent is
 * running here or the session never touched a cache. */
function PaneCacheInfo({ session, now }: { session: SessionData; now: number }) {
  const d = session.agentState?.details;
  // Gate on a live Claude in this pane: `agentState` is pruned when the pid
  // dies, so `isAgent && live` == "Claude running here right now".
  if (!session.live || !isAgent(session) || !d?.cacheExpiresAt) return null;
  const cold = isCold(d, now);
  const expiring = isCacheExpiring(d, now);
  return (
    <span
      title={
        cold
          ? "prompt cache expired"
          : expiring
            ? "prompt cache expires soon — any message re-warms it; a cold resume re-reads everything at full price"
            : "prompt cache warm — time left"
      }
      className={cn(
        "shrink-0 font-mono text-[10.5px]",
        expiring
          ? "text-amber-500"
          : cold
            ? "font-medium text-sky-500"
            : "text-muted-foreground/70",
      )}
    >
      {cold
        ? "❄ cache cold"
        : `${d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"} ${fmtMins(d.cacheExpiresAt - now)} left`}
    </span>
  );
}

/** Blocks the terminal until the user deliberately acknowledges a cold prompt
 * cache: unlike the quiet ❄ in the pane header, a cold resume silently
 * re-reads the whole transcript at full price, so this earns a click rather
 * than a glance. The ❄ pulses to draw the eye across a busy multi-pane grid;
 * the card itself stays put so the buttons are always easy to hit. Re-arms on
 * the next cold generation (keyed by `cacheExpiresAt`, the same dedup key the
 * board-wide toast in `screens/agentboard.tsx` uses). */
export function ColdCacheOverlay({
  session,
  now,
  onCompact,
}: {
  session: SessionData;
  now: number;
  /** Sends `/compact` to the session in place of acknowledging. */
  onCompact: () => void;
}) {
  const d = session.agentState?.details;
  const [ackedFor, setAckedFor] = useState<number | null>(null);
  const cold = session.live && isAgent(session) && !!d?.cacheExpiresAt && isCold(d, now);
  if (!cold || ackedFor === d!.cacheExpiresAt) return null;
  return (
    <div className="absolute inset-0 z-20 flex items-center justify-center bg-background/90 p-4">
      <div className="flex max-w-64 flex-col items-center gap-3 rounded-lg border-2 border-sky-500 bg-card px-5 py-4 text-center shadow-lg">
        <span className="animate-pulse text-2xl text-sky-500">❄</span>
        <div className="flex flex-col gap-1">
          <span className="text-sm font-medium text-foreground">prompt cache is cold</span>
          <span className="text-xs text-muted-foreground">
            resuming re-reads the full transcript at full price — any message re-warms it
          </span>
          {/* What that re-read would actually cost: which model, and how much
              context it would re-send — the two facts the compact-or-continue
              decision below turns on. */}
          {modelContextLabel(d) && (
            <span className="mt-0.5 font-mono text-[10.5px] text-muted-foreground/70">
              {modelContextLabel(d)}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={onCompact}
            className="rounded-md border border-sky-500/50 bg-sky-500/10 px-2.5 py-1 text-xs font-medium text-sky-500 hover:bg-sky-500/20"
          >
            /compact instead
          </button>
          <button
            type="button"
            autoFocus
            onClick={() => setAckedFor(d!.cacheExpiresAt!)}
            className="rounded-md border bg-background px-2.5 py-1 text-xs font-medium text-foreground hover:bg-accent"
          >
            got it — continue
          </button>
        </div>
      </div>
    </div>
  );
}

/** One pane's chrome: glyph · dot · session name · shell kind · running time ·
 * waiting age · cache info · lifecycle buttons. The repo / folder / branch /
 * diff live once in the working-context band above (every pane in a window
 * shares that folder), so they're not repeated here — the pane header only
 * identifies *which session* this is and how it's doing, mirroring the same
 * badges the rail row shows (`fmtElapsed`, `fmtWaitingAge`) so the two
 * surfaces never disagree. */
export function PaneHeader({
  session,
  label,
  now,
  actions,
}: {
  session: SessionData;
  label: string;
  now: number;
  actions: SessionActions;
}) {
  const agent = isAgent(session) && session.live;
  const waitingAge = fmtWaitingAge(session.needsSinceMs, now);
  return (
    <PaneChrome
      lens={
        <>
          <PaneLens
            kind={isAgent(session) ? "agent" : "shell"}
            label={isAgent(session) ? undefined : (session.shellKind ?? undefined)}
          />
          <Dot session={session} />
        </>
      }
      subject={label}
      subjectTitle={label}
      actions={
        <>
          {session.live && (
            <span
              className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70"
              title="running for"
            >
              {fmtElapsed(now - session.createdAt)}
            </span>
          )}
          {waitingAge && (
            <span
              className="shrink-0 font-mono text-[10.5px] text-amber-500/80"
              title="how long this has been needing you"
            >
              {waitingAge}
            </span>
          )}
          <PaneCacheInfo session={session} now={now} />
          {agent && (
            <IconBtn
              title="stop Claude (shell survives)"
              onClick={() => actions.stopClaude(session)}
              className="hover:text-red-500"
            >
              ■
            </IconBtn>
          )}
          <IconBtn
            title="close session (kills the PTY, drops the record)"
            onClick={() => actions.close(session.id)}
            className="hover:text-red-500"
          >
            <X className="size-3" />
          </IconBtn>
        </>
      }
    />
  );
}
