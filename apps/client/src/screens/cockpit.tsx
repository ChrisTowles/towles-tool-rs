import { useEffect, useState } from "react";
import {
  CalendarClock,
  CircleCheck,
  CircleDot,
  CircleX,
  Clock,
  ExternalLink,
  GitPullRequest,
  CircleAlert,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  fmtAge,
  fmtClock,
  fmtCountdown,
  useStoreSnapshot,
  type IssueItem,
  type PrItem,
} from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";

/**
 * Cockpit — the day home. One dense screen: how long until the next meeting, the
 * PRs that need you, and the issue queue across repos. Read-only over the store
 * snapshot; the countdown ticks every 30s.
 */
export function CockpitScreen() {
  const { snapshot, live } = useStoreSnapshot();
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  const upcoming = snapshot.events
    .filter((e) => e.startTs > now)
    .sort((a, b) => a.startTs - b.startTs);
  const nextEvent = upcoming[0];
  const later = upcoming.slice(1, 3);
  const soon = nextEvent && nextEvent.startTs - now < 15 * 60_000;

  const needsYouPrs = snapshot.prs.filter(
    (p) => p.checks === "failing" || p.reviewState === "review_requested",
  );
  const repos = new Set([
    ...snapshot.prs.map((p) => p.repo),
    ...snapshot.issues.map((i) => i.repo),
  ]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Next-meeting strip */}
      <div className="flex shrink-0 flex-wrap items-center gap-x-8 gap-y-2 border-b px-5 py-4">
        <div className="flex items-center gap-3">
          <CalendarClock
            className={cn("size-5", soon ? "text-amber-500" : "text-muted-foreground")}
          />
          {nextEvent ? (
            <div className="flex items-baseline gap-3">
              <span
                className={cn(
                  "font-mono text-3xl font-semibold tabular-nums",
                  soon ? "text-amber-500" : "text-foreground",
                )}
              >
                {fmtCountdown(nextEvent.startTs - now)}
              </span>
              <div className="flex flex-col">
                <span className="text-sm font-medium">{nextEvent.title}</span>
                <span className="text-xs text-muted-foreground">
                  {fmtClock(nextEvent.startTs)}
                  {nextEvent.location ? ` · ${nextEvent.location}` : ""}
                </span>
              </div>
            </div>
          ) : (
            <span className="text-sm text-muted-foreground">No more meetings today.</span>
          )}
        </div>

        {later.length > 0 && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className="uppercase tracking-wide">Then</span>
            {later.map((e) => (
              <span key={e.id} className="rounded-md bg-muted px-2 py-0.5">
                {e.title} · {fmtClock(e.startTs)}
              </span>
            ))}
          </div>
        )}

        <div className="ml-auto flex items-center gap-4 text-xs text-muted-foreground">
          <Gauge n={needsYouPrs.length} label="PRs need you" tone={needsYouPrs.length ? "warn" : "muted"} />
          <Gauge n={snapshot.issues.length} label="Issues" tone="muted" />
          <Gauge n={repos.size} label="Repos" tone="muted" />
        </div>
      </div>

      {!live && (
        <div className="flex shrink-0 items-center gap-2 border-b bg-amber-500/10 px-5 py-1.5 text-xs text-amber-700 dark:text-amber-400">
          <CircleAlert className="size-3.5 shrink-0" />
          Not connected to the store — open this window in the Towles Tool app to see live PRs,
          issues, and events.
        </div>
      )}

      <ScrollArea className="min-h-0 flex-1">
        <div className="grid grid-cols-1 gap-4 p-4 lg:grid-cols-2">
          {/* Pull requests */}
          <Panel
            title="Pull requests"
            note={`${needsYouPrs.length} need you`}
            icon={<GitPullRequest className="size-4" />}
          >
            {snapshot.prs.length === 0 ? (
              <Empty>{live ? "No open PRs across your repos." : "Not connected yet."}</Empty>
            ) : (
              snapshot.prs
                .slice()
                .sort((a, b) => rank(b) - rank(a) || b.updatedTs - a.updatedTs)
                .map((pr) => <PrRow key={`${pr.repo}#${pr.number}`} pr={pr} now={now} />)
            )}
          </Panel>

          {/* Issue queue */}
          <Panel
            title="Issue queue"
            note={`${snapshot.issues.length} open`}
            icon={<CircleDot className="size-4" />}
          >
            {snapshot.issues.length === 0 ? (
              <Empty>{live ? "No issues assigned to you." : "Not connected yet."}</Empty>
            ) : (
              snapshot.issues
                .slice()
                .sort((a, b) => b.updatedTs - a.updatedTs)
                .map((issue) => <IssueRow key={`${issue.repo}#${issue.number}`} issue={issue} now={now} />)
            )}
          </Panel>
        </div>
      </ScrollArea>
    </div>
  );
}

/** PR ordering weight: failing checks outrank review-requested outrank the rest. */
function rank(pr: PrItem): number {
  if (pr.checks === "failing") return 2;
  if (pr.reviewState === "review_requested") return 1;
  return 0;
}

function Gauge({ n, label, tone }: { n: number; label: string; tone: "warn" | "muted" }) {
  return (
    <div className="flex flex-col items-center">
      <span
        className={cn(
          "font-mono text-xl font-semibold tabular-nums",
          tone === "warn" && n > 0 ? "text-amber-500" : "text-foreground",
        )}
      >
        {n}
      </span>
      <span className="text-[10px] uppercase tracking-wide text-muted-foreground">{label}</span>
    </div>
  );
}

function Panel({
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

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="px-3 py-8 text-center text-sm text-muted-foreground">{children}</p>;
}

function ChecksIcon({ checks }: { checks: string }) {
  if (checks === "passing")
    return <CircleCheck className="size-4 shrink-0 text-green-600 dark:text-green-500" />;
  if (checks === "failing") return <CircleX className="size-4 shrink-0 text-destructive" />;
  if (checks === "none") return <CircleDot className="size-4 shrink-0 text-muted-foreground/50" />;
  return <Clock className="size-4 shrink-0 text-amber-600 dark:text-amber-500" />;
}

function PrRow({ pr, now }: { pr: PrItem; now: number }) {
  const reviewRequested = pr.reviewState === "review_requested";
  return (
    <a
      href={pr.url}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => {
        e.preventDefault();
        void openExternalUrl(pr.url);
      }}
      className="group flex items-center gap-3 px-3 py-2.5 text-sm hover:bg-accent/40"
    >
      <ChecksIcon checks={pr.checks} />
      <div className="min-w-0 flex-1">
        <div className="truncate">{pr.title}</div>
        <div className="truncate font-mono text-xs text-muted-foreground">
          {pr.repo} #{pr.number} · {fmtAge(pr.updatedTs, now)}
        </div>
      </div>
      {reviewRequested && (
        <Badge className="shrink-0 bg-blue-500/15 text-blue-700 dark:bg-blue-500/20 dark:text-blue-400">
          review you
        </Badge>
      )}
      {pr.checks === "failing" && (
        <Badge className="shrink-0 bg-red-500/15 text-red-700 dark:bg-red-500/20 dark:text-red-400">
          <CircleAlert className="size-3" /> checks
        </Badge>
      )}
      <ExternalLink className="size-3.5 shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100" />
    </a>
  );
}

function IssueRow({ issue, now }: { issue: IssueItem; now: number }) {
  return (
    <a
      href={issue.url}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => {
        e.preventDefault();
        void openExternalUrl(issue.url);
      }}
      className="group flex items-center gap-3 px-3 py-2.5 text-sm hover:bg-accent/40"
    >
      <CircleDot className="size-4 shrink-0 text-green-600 dark:text-green-500" />
      <div className="min-w-0 flex-1">
        <div className="truncate">{issue.title}</div>
        <div className="truncate font-mono text-xs text-muted-foreground">
          {issue.repo} #{issue.number} · {fmtAge(issue.updatedTs, now)}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {issue.labels.slice(0, 2).map((l) => (
          <Badge key={l} variant="outline" className="text-[10px]">
            {l}
          </Badge>
        ))}
      </div>
      <ExternalLink className="size-3.5 shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100" />
    </a>
  );
}
