import { CircleCheck, CircleDot, CircleX, Clock, ExternalLink } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { fmtAge, type CollectRun, type IssueItem, type PrItem } from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";

/**
 * Shared atoms for screens rendering store-snapshot data (Cockpit, Pull
 * requests, Config). One home so the PR/issue row anatomy and the collector
 * freshness line can't drift between screens.
 */

export function Panel({
  title,
  note,
  icon,
  children,
}: {
  title: string;
  note?: string;
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col overflow-hidden rounded-lg border">
      <div className="flex items-center justify-between border-b bg-muted/40 px-3 py-2">
        <div className="flex items-center gap-2 text-sm font-medium">
          {icon}
          {title}
        </div>
        {note && <span className="text-xs text-muted-foreground">{note}</span>}
      </div>
      <div className="flex flex-col divide-y">{children}</div>
    </section>
  );
}

export function Empty({ children }: { children: React.ReactNode }) {
  return <p className="px-3 py-8 text-center text-sm text-muted-foreground">{children}</p>;
}

/**
 * CI check-rollup badge for a PR row. One variant per collector state
 * (`passing | failing | pending | none`); unknown strings render as pending so
 * a new collector value degrades visibly instead of vanishing.
 */
export function ChecksBadge({ checks }: { checks: string }) {
  if (checks === "passing")
    return (
      <Badge className="shrink-0 bg-green-500/15 text-green-700 dark:bg-green-500/20 dark:text-green-400">
        <CircleCheck className="size-3" /> passing
      </Badge>
    );
  if (checks === "failing")
    return (
      <Badge className="shrink-0 bg-red-500/15 text-red-700 dark:bg-red-500/20 dark:text-red-400">
        <CircleX className="size-3" /> failing
      </Badge>
    );
  if (checks === "none")
    return (
      <Badge className="shrink-0 bg-muted text-muted-foreground">
        <CircleDot className="size-3" /> no checks
      </Badge>
    );
  return (
    <Badge className="shrink-0 bg-amber-500/15 text-amber-700 dark:bg-amber-500/20 dark:text-amber-400">
      <Clock className="size-3" /> pending
    </Badge>
  );
}

/** PR ordering weight: failing checks outrank review-requested outrank the rest. */
export function prRank(pr: PrItem): number {
  if (pr.state !== "merged" && pr.checks === "failing") return 2;
  if (pr.reviewState === "review_requested") return 1;
  return 0;
}

/** Whether a PR demands the owner's attention (mirrors the day-bar math). */
export function prNeedsYou(pr: PrItem): boolean {
  const checksFailing = pr.state !== "merged" && pr.checks === "failing";
  return checksFailing || pr.reviewState === "review_requested";
}

/**
 * One pull-request row. The title/meta area is the click target (opens the PR in
 * the browser); `actions`, when supplied, renders a trailing control (e.g.
 * Cockpit's per-PR dropdown menu) that lives *outside* the anchor so nested
 * interactive elements stay valid. Without it, a hover-revealed external-link
 * glyph stands in.
 */
export function PrRow({
  pr,
  now,
  actions,
}: {
  pr: PrItem;
  now: number;
  actions?: React.ReactNode;
}) {
  const reviewRequested = pr.reviewState === "review_requested";
  return (
    <div
      data-focus-kind="pr"
      data-focus-id={`${pr.repo}#${pr.number}`}
      className="group flex items-center gap-3 px-3 py-2.5 text-sm hover:bg-accent/40"
    >
      <a
        href={pr.url}
        target="_blank"
        rel="noreferrer"
        onClick={(e) => {
          e.preventDefault();
          void openExternalUrl(pr.url);
        }}
        className="flex min-w-0 flex-1 items-center gap-3"
      >
        <div className="min-w-0 flex-1">
          <div className="truncate">{pr.title}</div>
          <div className="truncate font-mono text-xs text-muted-foreground">
            {pr.repo} #{pr.number} · {fmtAge(pr.updatedTs, now)}
          </div>
        </div>
      </a>
      {reviewRequested && (
        <Badge className="shrink-0 bg-blue-500/15 text-blue-700 dark:bg-blue-500/20 dark:text-blue-400">
          review you
        </Badge>
      )}
      <ChecksBadge checks={pr.checks} />
      {actions ?? (
        <ExternalLink className="size-3.5 shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100" />
      )}
    </div>
  );
}

/**
 * One issue-queue row. The title/meta area is the click target (opens the issue
 * in the browser); `actions`, when supplied, renders a trailing control (e.g.
 * Cockpit's per-issue dropdown menu) that lives *outside* the anchor so nested
 * interactive elements stay valid. Without it, a hover-revealed external-link
 * glyph stands in.
 */
export function IssueRow({
  issue,
  now,
  actions,
}: {
  issue: IssueItem;
  now: number;
  actions?: React.ReactNode;
}) {
  return (
    <div className="group flex items-center gap-3 px-3 py-2.5 text-sm hover:bg-accent/40">
      <a
        href={issue.url}
        target="_blank"
        rel="noreferrer"
        onClick={(e) => {
          e.preventDefault();
          void openExternalUrl(issue.url);
        }}
        className="flex min-w-0 flex-1 items-center gap-3"
      >
        <CircleDot className="size-4 shrink-0 text-green-600 dark:text-green-500" />
        <div className="min-w-0 flex-1">
          <div className="truncate">{issue.title}</div>
          <div className="truncate font-mono text-xs text-muted-foreground">
            {issue.repo} #{issue.number} · {fmtAge(issue.updatedTs, now)}
          </div>
        </div>
      </a>
      <div className="flex shrink-0 items-center gap-1">
        {issue.labels.slice(0, 2).map((l) => (
          <Badge key={l} variant="outline" className="text-[10px]">
            {l}
          </Badge>
        ))}
      </div>
      {actions ?? (
        <ExternalLink className="size-3.5 shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100" />
      )}
    </div>
  );
}

/**
 * One collector's freshness, from the store's run bookkeeping. Green age when
 * the last run succeeded, red with the error when it failed, muted "never"
 * before the first run.
 */
export function CollectorFreshness({
  run,
  now,
}: {
  run: CollectRun | undefined;
  now: number;
}) {
  if (!run) {
    return <span className="font-mono text-[11px] text-muted-foreground/60">never ran</span>;
  }
  if (!run.ok) {
    return (
      <span
        className="truncate font-mono text-[11px] text-red-600 dark:text-red-500"
        title={run.message}
      >
        failed {fmtAge(run.ranAt, now)}
        {run.message ? ` · ${run.message}` : ""}
      </span>
    );
  }
  return (
    <span className="font-mono text-[11px] text-muted-foreground">
      ran {fmtAge(run.ranAt, now)}
      {run.message ? ` · ${run.message}` : ""}
    </span>
  );
}
