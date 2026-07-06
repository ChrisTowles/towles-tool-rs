import { FolderGit2 } from "lucide-react";
import {
  DiffButton,
  Dot,
  fmtMins,
  Glyph,
  IconBtn,
  PrChip,
  PurposeRow,
  WorktreeBadge,
} from "@/components/agentboard-bits";
import {
  isAgent,
  isCold,
  pathScope,
  type FolderData,
  type RepoData,
  type SessionActions,
  type SessionData,
} from "@/lib/agentboard";
import type { PrItem } from "@/lib/data";

/** The working-context band atop the main pane: *where am I working and why*.
 * Leads with the focused checkout name — large, first, the anchor of the whole
 * screen — with the repo and branch (plus git facts: worktree badge, diff
 * button, PR chip) on a quieter line below it, then the folder's purpose line.
 * One glance answers which checkout the terminals below belong to and what you
 * set out to do there. */
export function WorkingContext({
  repo,
  folder,
  pr,
  onOpenDiff,
}: {
  repo: RepoData;
  folder: FolderData;
  pr?: PrItem;
  onOpenDiff: (dir: string, name: string) => void;
}) {
  const scope = pathScope(folder.dir);
  // A slot/worktree has a distinct checkout name; a lone clone shares the
  // repo's, so we don't repeat it on the line below.
  const repoDistinct = folder.name !== repo.name;
  return (
    <div className="flex items-start gap-3 border-b bg-card px-4 py-2.5">
      <FolderGit2 className="mt-0.5 size-5 shrink-0 text-violet-500" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {/* Line 1: the checkout — largest, first. */}
        <span className="truncate text-2xl font-semibold leading-tight">{folder.name}</span>
        {/* Line 2: repo · branch + git facts, quieter. */}
        <div className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
          {scope && <span className="shrink-0 font-mono text-muted-foreground/60">{scope}</span>}
          {repoDistinct && <span className="shrink-0 font-medium">{repo.name}</span>}
          <span className="min-w-0 shrink truncate font-mono text-[11px]">⎇ {folder.branch}</span>
          {folder.isWorktree && <WorktreeBadge />}
          <DiffButton stats={folder} onOpen={() => onOpenDiff(folder.dir, folder.name)} />
          {pr && <PrChip pr={pr} />}
        </div>
        <PurposeRow folder={folder} variant="band" />
      </div>
    </div>
  );
}

/** Cache health for one pane, shown only while Claude is actually running in
 * it (a live agent for this session). Cache warmth only — no context percent —
 * so the pane chrome stays quiet: `⧗ 42m left` / `◔ 3m left` while warm,
 * `❄ cache cold` once the prompt cache has expired. Nothing when no agent is
 * running here or the session never touched a cache. */
function PaneCacheInfo({ session, now }: { session: SessionData; now: number }) {
  const d = session.agentState?.details;
  // Gate on a live Claude in this pane: `agentState` is pruned when the pid
  // dies, so `isAgent && live` == "Claude running here right now".
  if (!session.live || !isAgent(session) || !d?.cacheExpiresAt) return null;
  const cold = isCold(d, now);
  return (
    <span
      title={cold ? "prompt cache expired" : "prompt cache warm — time left"}
      className="shrink-0 font-mono text-[10.5px] text-muted-foreground/70"
    >
      {cold
        ? "❄ cache cold"
        : `${d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"} ${fmtMins(d.cacheExpiresAt - now)} left`}
    </span>
  );
}

/** One pane's chrome: glyph · dot · name · repo/folder⎇branch · diff · cache
 * info · ⊟. */
export function PaneHeader({
  session,
  folder,
  repoName,
  label,
  now,
  actions,
  onUngroup,
  onOpenDiff,
}: {
  session: SessionData;
  folder?: FolderData;
  repoName?: string;
  label: string;
  now: number;
  actions: SessionActions;
  onUngroup: () => void;
  onOpenDiff: (dir: string, name: string) => void;
}) {
  const agent = isAgent(session) && session.live;
  return (
    <div className="flex shrink-0 items-center gap-2 border-b bg-card px-2 py-1">
      <Glyph agent={isAgent(session)} />
      <Dot session={session} />
      <span className="truncate text-xs text-foreground">{label}</span>
      {folder && (
        <span className="truncate font-mono text-[10px] text-muted-foreground">
          {repoName && repoName !== folder.name ? `${repoName} / ${folder.name}` : folder.name} ⎇{" "}
          {folder.branch}
        </span>
      )}
      {folder?.isWorktree && <WorktreeBadge />}
      {folder && (
        <DiffButton stats={folder} onOpen={() => onOpenDiff(folder.dir, folder.name)} />
      )}
      <span className="ml-auto flex shrink-0 items-center gap-1.5">
        <PaneCacheInfo session={session} now={now} />
        {agent && (
          <IconBtn title="stop Claude (shell survives)" onClick={() => actions.stopClaude(session)} className="hover:text-red-500">
            ■
          </IconBtn>
        )}
        <IconBtn title="remove pane (session stays in the rail)" onClick={onUngroup} className="hover:text-sky-500">
          ⊟
        </IconBtn>
        <IconBtn title="kill session (PTY + record)" onClick={() => actions.close(session.id)} className="hover:text-red-500">
          ✕
        </IconBtn>
      </span>
    </div>
  );
}
