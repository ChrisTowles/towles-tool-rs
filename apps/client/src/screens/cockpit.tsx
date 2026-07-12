import { useEffect, useState } from "react";
import { CalendarClock, CircleAlert, CircleDot, GitPullRequest, Video } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  currentOrNextEvent,
  eventIsLive,
  fmtClock,
  fmtCountdown,
  useStoreSnapshot,
} from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";
import { Empty, IssueRow, Panel, PrRow, prNeedsYou, prRank } from "@/components/store-bits";

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

  const nextEvent = currentOrNextEvent(snapshot.events, now);
  const meetingLive = nextEvent ? eventIsLive(nextEvent, now) : false;
  const soon = nextEvent ? !meetingLive && nextEvent.startTs - now < 15 * 60_000 : false;
  // Highlight amber while a meeting is live or imminent.
  const highlight = meetingLive || soon;
  const later = snapshot.events
    .filter((e) => e.startTs > now && e.id !== nextEvent?.id)
    .sort((a, b) => a.startTs - b.startTs)
    .slice(0, 2);

  const needsYouPrs = snapshot.prs.filter(prNeedsYou);
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
            className={cn("size-5", highlight ? "text-amber-500" : "text-muted-foreground")}
          />
          {nextEvent ? (
            <div className="flex items-center gap-3">
              <span
                className={cn(
                  "font-mono text-3xl font-semibold tabular-nums",
                  highlight ? "text-amber-500" : "text-foreground",
                )}
              >
                {meetingLive ? "Now" : fmtCountdown(nextEvent.startTs - now)}
              </span>
              <div className="flex min-w-0 flex-col">
                <span className="text-sm font-medium">{nextEvent.title}</span>
                <span className="text-xs text-muted-foreground">
                  {meetingLive && nextEvent.endTs !== undefined
                    ? `until ${fmtClock(nextEvent.endTs)}`
                    : fmtClock(nextEvent.startTs)}
                  {nextEvent.location ? ` · ${nextEvent.location}` : ""}
                </span>
              </div>
              {nextEvent.joinUrl ? (
                <Button
                  size="sm"
                  variant={meetingLive ? "default" : "outline"}
                  className={cn(
                    meetingLive &&
                      "bg-amber-500 text-white hover:bg-amber-500/90 dark:bg-amber-500 dark:text-white",
                  )}
                  onClick={() => {
                    if (nextEvent.joinUrl) void openExternalUrl(nextEvent.joinUrl);
                  }}
                >
                  <Video />
                  Join
                </Button>
              ) : null}
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
                .sort((a, b) => prRank(b) - prRank(a) || b.updatedTs - a.updatedTs)
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

