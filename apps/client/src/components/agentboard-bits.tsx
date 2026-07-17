import { useState, type ComponentProps, type ReactNode } from "react";
import {
  Check,
  ChevronDown,
  CircleDot,
  Files,
  FolderPlus,
  GitCompare,
  GitPullRequest,
  Loader2,
  MoreVertical,
  RefreshCw,
  StickyNote,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { Input } from "@/components/ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { toast } from "sonner";
import {
  abCreateIssue,
  abInvoke,
  abSyncRepo,
  comparedBaseLabel,
  ctxPct,
  isCacheExpiring,
  isCold,
  needsCompact,
  prMergedButFolderHasWork,
  statusColor,
  type AgentStatus,
  type CommitStat,
  type FolderData,
  type FolderMetadata,
  type MetadataTone,
  type PortDrift,
  type SessionData,
} from "@/lib/agentboard";
import type { PrItem } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";
import { cn } from "@/lib/utils";

/**
 * Shared atoms for the Agentboard UI — one visual language for the rail rows,
 * folder headers, pane chrome, and the working-context band, so each surface
 * composes the same pieces instead of hand-rolling its own variants.
 */

/** A small square icon action that *reads as a button* (bordered, hover fill)
 * — shadcn outline button at icon-xs, mono glyph or lucide icon inside.
 * `title` renders as a real (Radix) tooltip: instant, styled, and — unlike a
 * native `title` attribute or CSS `:hover` reveal — reliable in the Tauri
 * WebKitGTK webview. It doubles as the `aria-label`, since the glyph alone
 * says nothing. Clicks never bubble into the row/header the button sits on. */
export function IconBtn({
  title,
  onClick,
  className,
  children,
  ...props
}: {
  title: string;
  onClick: () => void;
  className?: string;
  children: ReactNode;
} & Omit<ComponentProps<"button">, "onClick" | "title" | "children">) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="outline"
          size="icon-xs"
          aria-label={title}
          onClick={(e) => {
            e.stopPropagation();
            onClick();
          }}
          className={cn("font-mono text-xs text-muted-foreground", className)}
          {...props}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent side="bottom">{title}</TooltipContent>
    </Tooltip>
  );
}

/** ✦ for an agent session, ❯ for a plain shell. */
export function Glyph({ agent }: { agent: boolean }) {
  return (
    <span
      className={cn(
        "w-4 shrink-0 text-center font-mono text-xs",
        agent ? "text-violet-500" : "text-muted-foreground/60",
      )}
    >
      {agent ? "✦" : "❯"}
    </span>
  );
}

/** Status dot mirroring `statusColor`; pulses while busy. A session with no
 * live PTY shows a hollow ring — the record exists but nothing is running.
 * "Look at this" is the row's amber border (`sessionCatchesEye`), not the
 * dot — a resting board stays still. */
export function Dot({ session }: { session: SessionData }) {
  if (!session.live) {
    return (
      <span
        title="not started"
        className="size-2 shrink-0 rounded-full border-[1.5px] border-muted-foreground/50 bg-transparent"
      />
    );
  }
  const st = session.agentState?.status;
  return (
    <span
      title={st ? `agent ${st}` : "shell running, no agent"}
      className={cn(
        "size-2 shrink-0 rounded-full",
        st ? statusColor(st) : "bg-muted-foreground/40",
        st === "busy" && "animate-pulse",
      )}
    />
  );
}

/** A status-colored micro-dot + count, e.g. "●3", for agent rollups (the rail
 * chip and the nav sidebar). Color always derives from `statusColor` so the
 * buckets can never drift from the `Dot` atom. */
export function DotCount({ status, n }: { status: AgentStatus; n: number }) {
  return (
    <span className="flex items-center gap-1 text-muted-foreground">
      <span className={cn("size-1.5 rounded-full", statusColor(status))} />
      {n}
    </span>
  );
}

export function Chevron({ collapsed }: { collapsed: boolean }) {
  return (
    <ChevronDown
      className={cn(
        "size-3.5 shrink-0 text-muted-foreground transition-transform",
        collapsed && "-rotate-90",
      )}
    />
  );
}

export function NeedsBadge({ n, className }: { n: number; className?: string }) {
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

/** Marks a tracked checkout whose directory no longer exists on disk — a
 * "ghost". Deliberately grayscale (dashed, dimmed): a missing dir is a dead
 * state, not the live-attention amber the needs-you math owns, so it reads as
 * "gone/inert" rather than "look at me". Grayscale tokens carry light + dark. */
export function GhostBadge() {
  return (
    <span
      className="shrink-0 rounded-md border border-dashed border-muted-foreground/40 px-1 font-mono text-[10px] text-muted-foreground/70"
      title="This checkout's directory is gone (moved or deleted). Untrack it, or restore the directory to bring it back."
    >
      ⚠ missing
    </span>
  );
}

/** Marks a folder as a git worktree checkout (linked to another checkout's
 * `.git`) — distinct from the `p/`/`w/` path-scope prefix, so a worktree's
 * WIP diff doesn't read as the repo's one canonical state. */
export function WorktreeBadge() {
  return (
    <span
      className="shrink-0 rounded-md border border-sky-500/40 bg-sky-500/10 px-1 font-mono text-[10px] text-sky-500"
      title="Git worktree checkout — a linked working tree, not the primary clone"
    >
      ⬡ wt
    </span>
  );
}

/** Shown on a worktree checkout mid-delete (`slot_remove` in flight). The rail
 * row itself dims and goes `pointer-events-none` around this badge (see
 * `RepoGroup`'s `deletingDirs`/`FolderHeader`'s `deleting` prop) — this is
 * just the label explaining *why* the row went inert, same job `GhostBadge`
 * does for a missing directory. Red (not the neutral gray of `GhostBadge`):
 * unlike a ghost, which is passively gone, this is an active, irreversible
 * deletion in progress. */
export function DeletingBadge() {
  return (
    <span
      className="flex shrink-0 items-center gap-1 rounded-md border border-red-500/40 bg-red-500/10 px-1 font-mono text-[10px] text-red-600 dark:text-red-400"
      title="Deleting this worktree from disk…"
    >
      <Loader2 className="size-2.5 animate-spin" /> deleting…
    </span>
  );
}

/** Marks a folder where a live pane's ports have drifted from what `.env`
 * currently claims — a sibling slot's re-render (or a manual `tt slot env`)
 * rotated a port this pane already bound to. Amber like `NeedsBadge`: unlike
 * the grayscale `GhostBadge`, this is something worth acting on (restart the
 * pane, or re-run `tt slot env` and restart whatever's bound to the stale
 * port), not a dead state. */
export function PortDriftBadge({ drift }: { drift: PortDrift[] }) {
  if (drift.length === 0) return null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="shrink-0 rounded-md border border-amber-500/50 bg-amber-500/10 px-1 font-mono text-[10px] text-amber-500">
          ⚡ port drift
        </span>
      </TooltipTrigger>
      <TooltipContent side="bottom" align="start">
        <div className="flex flex-col gap-0.5 font-mono text-[11px]">
          <span className="text-muted-foreground">
            {drift.length === 1 ? "A pane" : "Panes"} started before {drift.length === 1 ? "this" : "these"}{" "}
            port{drift.length === 1 ? "" : "s"} last changed — restart to pick up the current .env:
          </span>
          {drift.map((d) => (
            <span key={`${d.key}:${d.spawnedPort}:${d.currentPort}`}>
              {d.key} {d.spawnedPort} → {d.currentPort}
            </span>
          ))}
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

/** Which branch every git stat on this folder was measured against — `vs
 * main` or `vs docs/readme-slot-clean` for a slot with a different creation
 * base — next to the branch name so the ↑↓/±  numbers beside it are never
 * ambiguous about what they mean. */
export function ComparedBaseBadge({
  folder,
}: {
  folder: Pick<FolderData, "comparedBase" | "baseBranch" | "slotBaseBranch">;
}) {
  const label = comparedBaseLabel(folder);
  const manual = Boolean(folder.baseBranch?.trim());
  return (
    <span
      className="shrink-0 rounded-md border border-border/70 px-1 font-mono text-[10px] text-muted-foreground"
      title={
        manual
          ? `Diffs against "${label}" — your override for this folder`
          : folder.slotBaseBranch
            ? `Diffs against "${label}" — the ref this slot was created from`
            : `Diffs against "${label}" (origin/main-or-master auto-detect)`
      }
    >
      vs {label}
    </span>
  );
}

/** Commits ahead/behind `comparedBase`, next to the branch name — `↑3 ↓2`.
 * Ahead (unmerged local commits) reads emerald like a diff `+`; behind (just
 * staleness, not an attention signal) stays a muted amber. Renders nothing
 * when even with the compared branch. */
export function AheadBehind({
  stats,
}: {
  stats: Pick<FolderData, "commitsAhead" | "commitsBehind" | "comparedBase">;
}) {
  const { commitsAhead, commitsBehind } = stats;
  if (commitsAhead === 0 && commitsBehind === 0) return null;
  const base = comparedBaseLabel(stats);
  return (
    <span
      className="shrink-0 font-mono text-[10.5px]"
      title={`${commitsAhead} commit${commitsAhead === 1 ? "" : "s"} ahead of ${base}, ${commitsBehind} behind`}
    >
      {commitsAhead > 0 && (
        <span className="text-emerald-600 dark:text-emerald-400">↑{commitsAhead}</span>
      )}
      {commitsAhead > 0 && commitsBehind > 0 && " "}
      {commitsBehind > 0 && <span className="text-amber-600 dark:text-amber-400">↓{commitsBehind}</span>}
    </span>
  );
}

/** One row of the `DiffButton` hover's per-commit breakdown: short SHA,
 * truncated subject, and that commit's own ± tally. */
function CommitStatRow({ commit }: { commit: CommitStat }) {
  return (
    <div className="flex items-center gap-2 font-mono text-[10.5px] leading-tight">
      <span className="shrink-0 text-muted-foreground/70">{commit.sha.slice(0, 7)}</span>
      <span className="min-w-0 flex-1 truncate text-foreground">{commit.subject}</span>
      <span className="shrink-0 text-emerald-600 dark:text-emerald-400">
        +{commit.linesAdded}
      </span>
      <span className="shrink-0 text-red-600 dark:text-red-400">−{commit.linesRemoved}</span>
    </div>
  );
}

/** The per-commit breakdown inside `DiffButton`'s hover card: every commit
 * `comparedBase` doesn't have, oldest first, with its own ± tally, and a
 * total row at the bottom — a many-commit branch's ± tally isn't one
 * anonymous blob. The total is the folder's own `linesAdded`/`linesRemoved`
 * (not a sum of the rows above) since those also cover uncommitted work,
 * which never gets its own commit row. Fetched lazily (only once the card
 * actually opens) and cached for the folder's lifetime in the parent's
 * state. */
function CommitBreakdownPreview({
  commits,
  linesAdded,
  linesRemoved,
}: {
  commits: CommitStat[] | null;
  linesAdded: number;
  linesRemoved: number;
}) {
  if (commits == null) {
    return <p className="p-1 text-xs text-muted-foreground">loading commits…</p>;
  }
  return (
    <div className="max-h-80 overflow-auto">
      <div className="flex flex-col gap-1">
        {commits.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            no commits ahead — uncommitted changes only
          </p>
        ) : (
          commits.map((c) => <CommitStatRow key={c.sha} commit={c} />)
        )}
      </div>
      <div className="mt-1.5 flex items-center gap-2 border-t border-border/70 pt-1.5 font-mono text-[10.5px] font-semibold">
        <span className="min-w-0 flex-1 text-foreground">
          Total{commits.length > 0 && ` — ${commits.length} commit${commits.length === 1 ? "" : "s"}`}
        </span>
        <span className="shrink-0 text-emerald-600 dark:text-emerald-400">+{linesAdded}</span>
        <span className="shrink-0 text-red-600 dark:text-red-400">−{linesRemoved}</span>
      </div>
    </div>
  );
}

/** The diff entry point — a real, always-visible button (never hidden behind
 * a hover or dropped when the tree is clean, so the feature stays findable).
 * Clean folders read a quiet `diff`; dirty ones carry the commit count next
 * to the ± tally. Hovering previews the per-commit breakdown (each commit's
 * own ± tally, plus a total) so a branch with many commits doesn't force a
 * click just to see roughly what changed. */
export function DiffButton({
  stats,
  onOpen,
}: {
  stats: Pick<
    FolderData,
    | "dir"
    | "filesChanged"
    | "linesAdded"
    | "linesRemoved"
    | "commitsAhead"
    | "comparedBase"
    | "baseBranch"
  >;
  onOpen: () => void;
}) {
  const { dir, filesChanged, linesAdded, linesRemoved, commitsAhead, baseBranch } = stats;
  const clean = linesAdded === 0 && linesRemoved === 0;
  const base = comparedBaseLabel(stats);
  const [commits, setCommits] = useState<CommitStat[] | null>(null);

  return (
    <HoverCard
      openDelay={250}
      onOpenChange={(open) => {
        if (open && commits == null) {
          void abInvoke<CommitStat[]>("ab_get_commit_stats", {
            dir,
            baseBranch: baseBranch?.trim() || null,
          }).then((c) => setCommits(c ?? []));
        }
      }}
    >
      <HoverCardTrigger asChild>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onOpen();
          }}
          className="flex h-5 shrink-0 items-center gap-1 rounded-md border border-border/70 px-1.5 font-mono text-[10.5px] text-muted-foreground transition-colors hover:border-border hover:bg-accent hover:text-foreground"
          title={
            clean
              ? `No changes vs ${base} — view diff`
              : `${filesChanged} file${filesChanged === 1 ? "" : "s"} changed, ${commitsAhead} commit${commitsAhead === 1 ? "" : "s"} ahead of ${base} — view diff`
          }
        >
          <GitCompare className="size-3" />
          {clean ? (
            <span>diff</span>
          ) : (
            <>
              <span className="text-muted-foreground">{commitsAhead}c</span>
              <span className="text-emerald-600 dark:text-emerald-400">+{linesAdded}</span>
              <span className="text-red-600 dark:text-red-400">−{linesRemoved}</span>
            </>
          )}
        </button>
      </HoverCardTrigger>
      {!clean && (
        <HoverCardContent
          side="bottom"
          align="start"
          className="w-[28rem] max-w-[calc(100vw-2rem)]"
          onClick={(e) => e.stopPropagation()}
        >
          <CommitBreakdownPreview
            commits={commits}
            linesAdded={linesAdded}
            linesRemoved={linesRemoved}
          />
        </HoverCardContent>
      )}
    </HoverCard>
  );
}

/** The files entry point, DiffButton's sibling: opens the folder's full file
 * tree as a pane ("tell claude about any file"), always visible for the same
 * findability reason. */
export function FilesButton({ onOpen }: { onOpen: () => void }) {
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        onOpen();
      }}
      className="flex h-5 shrink-0 items-center gap-1 rounded-md border border-border/70 px-1.5 font-mono text-[10.5px] text-muted-foreground transition-colors hover:border-border hover:bg-accent hover:text-foreground"
      title="Browse every file in this checkout — @ any of them to Claude"
    >
      <Files className="size-3" />
      <span>files</span>
    </button>
  );
}

/** Precise reason a merged PR's checkout still isn't safe to delete — the two
 * conditions `folderSafeToDelete` checks, spelled out separately so the
 * tooltip never leaves you guessing which one is blocking it. Null once both
 * are satisfied (the caller has nothing left to warn about). */
function unsafeToDeleteReason(
  stats: Pick<FolderData, "dirty" | "commitsUnlanded">,
  base: string,
): string | null {
  const reasons: string[] = [];
  if (stats.dirty) reasons.push("has uncommitted changes");
  if (stats.commitsUnlanded > 0) {
    reasons.push(
      `has ${stats.commitsUnlanded} commit${stats.commitsUnlanded === 1 ? "" : "s"} that haven't landed on ${base} yet`,
    );
  }
  if (reasons.length === 0) return null;
  return `this checkout still ${reasons.join(" and ")}`;
}

/** Clickable `#N` chip for the folder's PR, tinted by its checks state (red
 * failing · green passing · yellow pending · gray none). Once merged the
 * chip normally turns purple — the slot is done, time to `tt slot rm` it —
 * but merged only means the *PR's* content is safe; it says nothing about
 * this checkout. If `stats` shows uncommitted changes or commits that
 * haven't landed on the base branch yet (`folderSafeToDelete`), the chip
 * turns amber (this app's needs-you hue) instead, since removing the slot
 * would lose that work despite the PR being merged — see the adjacent
 * `SafeToDeleteBadge` for the positive case. Opens GitHub. */
export function PrChip({
  pr,
  stats,
}: {
  pr: PrItem;
  stats: Pick<FolderData, "dirty" | "commitsUnlanded" | "comparedBase">;
}) {
  const merged = pr.state === "merged";
  const hasLocalWork = prMergedButFolderHasWork(pr, stats);
  const base = comparedBaseLabel(stats);
  const tone =
    hasLocalWork
      ? "border-amber-500/50 bg-amber-500/10 text-amber-600 hover:bg-amber-500/20 dark:text-amber-400"
      : merged
        ? "border-purple-500/50 bg-purple-500/10 text-purple-600 hover:bg-purple-500/20 dark:text-purple-400"
        : pr.checks === "failing"
          ? "border-red-500/50 bg-red-500/10 text-red-600 hover:bg-red-500/20 dark:text-red-400"
          : pr.checks === "passing"
            ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/20 dark:text-emerald-400"
            : pr.checks === "pending"
              ? "border-yellow-500/40 bg-yellow-500/10 text-yellow-600 hover:bg-yellow-500/20 dark:text-yellow-400"
              : "border-border/70 text-muted-foreground hover:bg-accent hover:text-foreground";
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        void openExternalUrl(pr.url);
      }}
      className={cn(
        "flex h-5 shrink-0 items-center gap-1 rounded-md border px-1.5 font-mono text-[10.5px] transition-colors",
        tone,
      )}
      title={
        hasLocalWork
          ? `${pr.title} — merged, but ${unsafeToDeleteReason(stats, base)}. Removing the slot would lose them — commit/push first. Open on GitHub.`
          : merged
            ? `${pr.title} — merged. Open on GitHub.`
            : `${pr.title} — checks ${pr.checks}${pr.reviewState === "review_requested" ? ", review requested" : ""}. Open on GitHub.`
      }
    >
      <GitPullRequest className="size-3" />#{pr.number}
      {hasLocalWork && <span aria-hidden>⚑</span>}
    </button>
  );
}

/** The positive counterpart to `PrChip`'s amber warning: a folder whose PR
 * merged, has no uncommitted changes, and has every commit landed on its
 * base — `folderSafeToDelete`. Deliberately louder than a bare chip (the bug
 * this replaces: a subdued purple "#N" was the *only* signal, indistinguishable
 * at a glance from an ordinary merged-but-still-active checkout). Emerald
 * (this app's "done/complete" hue — matches `statusColor`'s `complete` dot and
 * the diff `+` count) rather than the PR chip's purple, so it reads as a
 * distinct, actionable "you're done here" rather than another PR-state tint.
 * Clicking goes straight to the same guarded delete-worktree confirmation as
 * the folder's "···" menu — not a shortcut around it, just a louder path to
 * it, since this state is exactly when you'd want to take that action. */
export function SafeToDeleteBadge({
  base,
  onDeleteWorktree,
}: {
  base: string;
  onDeleteWorktree: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onDeleteWorktree();
          }}
          className="flex h-5 shrink-0 items-center gap-1 rounded-md border border-emerald-500/50 bg-emerald-500/10 px-1.5 font-mono text-[10.5px] text-emerald-600 transition-colors hover:bg-emerald-500/20 dark:text-emerald-400"
        >
          <Check className="size-3" /> safe to delete
        </button>
      </TooltipTrigger>
      <TooltipContent side="bottom" align="start">
        No uncommitted changes, and every commit has landed on {base}. Nothing here would be
        lost — click to delete this worktree.
      </TooltipContent>
    </Tooltip>
  );
}

/** Context/cache health for a live agent session, in the row's meta cluster.
 * Quiet mono text: `41% ◔4m` while warm (⧗ for a 1h cache), `41% ❄` when cold,
 * and an ice-washed `❄ 63% compact` pill when cold at/over the threshold. */
export function CacheBadge({
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
    // Pulses like the busy dot — a cold-and-huge session is a live nudge
    // ("compact this before you resume it"), not a passive fact.
    const pill =
      "shrink-0 animate-pulse rounded-md border border-sky-500/50 bg-sky-500/10 px-1.5 font-mono text-[10.5px] text-sky-500";
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

  const expiring = isCacheExpiring(d, now);
  const warmth = cold
    ? "❄"
    : `${d.cacheTtlMs === 3_600_000 ? "⧗" : "◔"}${fmtMins(d.cacheExpiresAt! - now)}`;
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
      {pct}% {warmth}
    </span>
  );
}

/** Millis → whole minutes for the cache countdown, floored at 1 ("<1m" ≈ 1m). */
export function fmtMins(ms: number): string {
  return `${Math.max(1, Math.round(ms / 60_000))}m`;
}

/** Text colors for agent-pushed status/log tones. Every hue carries a `dark:`
 * pair — never a bare palette color. */
const TONE_TEXT: Record<MetadataTone, string> = {
  neutral: "text-muted-foreground",
  info: "text-sky-600 dark:text-sky-400",
  success: "text-emerald-600 dark:text-emerald-400",
  warn: "text-amber-600 dark:text-amber-400",
  error: "text-red-600 dark:text-red-400",
};

/** The agent's own status line (`ab_set_status`, also pushed over MCP) under a
 * folder header — what the agent *says* it's doing, next to what we *observe*
 * (the session dots). Read-only by design; recent `ab_log` lines ride along in
 * the tooltip. Renders nothing when no agent has pushed a status. */
export function AgentStatusLine({
  metadata,
  now,
}: {
  metadata: FolderMetadata | null | undefined;
  now: number;
}) {
  const status = metadata?.status;
  if (!status?.text) return null;
  const tone = status.tone ?? "neutral";
  const logs = (metadata?.logs ?? []).slice(-5);
  const age = Math.max(0, now - status.ts);
  const line = (
    <span className={cn("flex min-w-0 items-center gap-1.5 text-[11px]", TONE_TEXT[tone])}>
      <span className="shrink-0 opacity-60">▸</span>
      <span className="min-w-0 truncate">{status.text}</span>
      {age >= 60_000 && (
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground/60">
          {fmtMins(age)} ago
        </span>
      )}
    </span>
  );
  if (logs.length === 0) return line;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{line}</TooltipTrigger>
      <TooltipContent side="bottom" align="start" className="max-w-96">
        <div className="flex flex-col gap-0.5 font-mono text-[11px]">
          {logs.map((l, i) => (
            <span key={i} className="truncate">
              {l.message}
            </span>
          ))}
        </div>
      </TooltipContent>
    </Tooltip>
  );
}

/** The folder's user-authored purpose — the "why am I here". Click to edit
 * inline (Enter saves, Esc cancels; blank clears).
 *
 * `rail` variant: a faint one-liner under the folder header, shown only when a
 * note is set. When unset it renders nothing at all (not even on hover) so the
 * folder row keeps a fixed height and the rail never jumps as the mouse moves
 * across it — set a note from the folder's "…" menu instead.
 * `band` variant: lives in the working-context band — always visible, unset
 * state included, because the band exists to answer "where am I and why". */
export function PurposeRow({
  folder,
  variant = "rail",
}: {
  folder: FolderData;
  variant?: "rail" | "band";
}) {
  const [editing, setEditing] = useState(false);
  const purpose = folder.purpose?.trim() ?? "";
  const rail = variant === "rail";
  const pad = rail ? "py-0.5 pr-3 pl-9 text-[11px]" : "text-xs";

  async function commit(text: string) {
    setEditing(false);
    const trimmed = text.trim();
    if (trimmed === purpose) return;
    await abInvoke("ab_set_folder_purpose", { dir: folder.dir, text: trimmed || null });
  }

  if (editing) {
    return (
      <div className={cn(rail && "py-0.5 pr-3 pl-9")}>
        <input
          autoFocus
          defaultValue={purpose}
          placeholder="what are you working toward here?"
          onBlur={(e) => void commit(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void commit((e.target as HTMLInputElement).value);
            if (e.key === "Escape") setEditing(false);
          }}
          className={cn(
            "w-full rounded-sm border border-input bg-background px-1.5 py-0.5 outline-none",
            rail ? "text-[11px]" : "text-xs",
          )}
        />
      </div>
    );
  }

  if (!purpose) {
    // Rail: no note → no row (a stable-height folder that doesn't resize on
    // hover). The note is set from the folder's "…" menu.
    if (rail) return null;
    return (
      <button
        type="button"
        onClick={() => setEditing(true)}
        title="Edit folder purpose"
        className={cn(
          "block w-full truncate text-left text-muted-foreground/50 hover:text-muted-foreground",
          pad,
        )}
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
      className={cn(
        "block w-full truncate text-left text-muted-foreground hover:text-foreground",
        pad,
      )}
    >
      {purpose}
    </button>
  );
}

/** "···" overflow menu for a checkout — the one place every secondary action
 * lives, shared verbatim by the rail's repo/folder headers and the
 * working-context band atop the panes (so the two surfaces never diverge):
 * full folder path (when given), "New slot…" (slot-convention repos),
 * "Delete worktree…" (worktree checkouts, guarded `slot_remove`),
 * "Set/Edit note…" (when a `folder` is given — the note shown under the
 * folder in the rail), "Create issue…" (shells `gh issue create` in `dir`),
 * and "Remove from rail". */
export function RepoMenu({
  path,
  onRemove,
  dir,
  folder,
  isWorktree,
  onNewSlot,
  onDeleteWorktree,
}: {
  path?: string;
  onRemove: () => void;
  dir: string;
  /** When set, the menu offers note editing for this checkout. */
  folder?: FolderData;
  /** Worktree checkouts have no "Remove from rail" — meaningless (they are
   * auto-discovered from the primary and would reappear next poll); deletion
   * is the "Delete worktree…" item instead. */
  isWorktree?: boolean;
  /** Opens the new-slot modal — set only on a slot-convention repo. */
  onNewSlot?: () => void;
  /** Deletes this worktree slot from disk (guarded, `slot_remove`) — set only
   * on worktree checkouts. */
  onDeleteWorktree?: () => void;
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

  async function syncNow() {
    try {
      const result = await abSyncRepo(dir);
      // `started: false` means a sync for this dir was already in flight
      // (e.g. another window) — quietly ignore rather than double-toast.
      if (!result.started) return;
      if (result.ok) {
        toast.success("Synced with GitHub");
      } else {
        toast.error(result.message ?? "Sync failed");
      }
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
            title="More actions"
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
          {onNewSlot && (
            <DropdownMenuItem onSelect={onNewSlot} className="whitespace-nowrap">
              <FolderPlus className="size-3.5" /> New slot…
            </DropdownMenuItem>
          )}
          {onDeleteWorktree && (
            <DropdownMenuItem
              variant="destructive"
              onSelect={onDeleteWorktree}
              className="whitespace-nowrap"
            >
              <Trash2 className="size-3.5" /> Delete worktree…
            </DropdownMenuItem>
          )}
          {(onNewSlot || onDeleteWorktree) && <DropdownMenuSeparator />}
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
          <DropdownMenuItem onSelect={() => void syncNow()} className="whitespace-nowrap">
            <RefreshCw className="size-3.5" /> Sync now
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => setIssueOpen(true)} className="whitespace-nowrap">
            <CircleDot className="size-3.5" /> Create issue…
          </DropdownMenuItem>
          {!isWorktree && (
            <DropdownMenuItem
              variant="destructive"
              onSelect={onRemove}
              className="whitespace-nowrap"
            >
              <Trash2 className="size-3.5" /> Remove from rail
            </DropdownMenuItem>
          )}
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
