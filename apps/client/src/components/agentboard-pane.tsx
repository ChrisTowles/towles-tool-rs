import { FolderGit2 } from "lucide-react";
import {
  CacheBadge,
  DiffButton,
  Dot,
  Glyph,
  IconBtn,
  PrChip,
  PurposeRow,
  WorktreeBadge,
} from "@/components/agentboard-bits";
import {
  isAgent,
  pathScope,
  type FolderData,
  type RepoData,
  type SessionActions,
  type SessionData,
} from "@/lib/agentboard";
import type { PrItem } from "@/lib/data";

/** The working-context band atop the main pane: *where am I working and why*.
 * Leads with the focused folder (violet = focus), its branch and git facts —
 * worktree badge, always-visible diff button, PR chip — then the folder's
 * purpose line. This is the control-center anchor: one glance answers which
 * checkout the terminals below belong to and what you set out to do there. */
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
  return (
    <div className="flex items-start gap-3 border-b bg-card px-4 py-2">
      <FolderGit2 className="mt-1 size-4 shrink-0 text-violet-500" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          {scope && (
            <span className="shrink-0 font-mono text-sm text-muted-foreground/60">{scope}</span>
          )}
          {/* Names hold their width; the branch is the flexible one that
              truncates when space runs out. */}
          <span className="shrink-0 text-sm font-semibold">{repo.name}</span>
          {folder.name !== repo.name && (
            <span className="shrink-0 text-sm font-medium text-muted-foreground">
              / {folder.name}
            </span>
          )}
          <span className="min-w-0 shrink truncate font-mono text-[11px] text-muted-foreground">
            ⎇ {folder.branch}
          </span>
          {folder.isWorktree && <WorktreeBadge />}
          <DiffButton stats={folder} onOpen={() => onOpenDiff(folder.dir, folder.name)} />
          {pr && <PrChip pr={pr} />}
        </div>
        <PurposeRow folder={folder} variant="band" />
      </div>
    </div>
  );
}

/** One pane's chrome: glyph · dot · name · repo/folder⎇branch · diff · cache
 * badge · ⊟. */
export function PaneHeader({
  session,
  folder,
  repoName,
  label,
  now,
  compactPct,
  actions,
  onUngroup,
  onOpenDiff,
}: {
  session: SessionData;
  folder?: FolderData;
  repoName?: string;
  label: string;
  now: number;
  compactPct: number;
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
        <CacheBadge
          session={session}
          now={now}
          compactPct={compactPct}
          onCompact={() => actions.compactClaude(session)}
          long
        />
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
