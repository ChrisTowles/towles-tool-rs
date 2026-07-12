import {
  CalendarClock,
  CircleAlert,
  CircleDot,
  ExternalLink,
  GitBranchPlus,
  GitPullRequest,
  MoreHorizontal,
  Send,
  Video,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  COUNTDOWN_SECONDS_THRESHOLD,
  currentOrNextEvent,
  eventIsLive,
  fmtClock,
  fmtCountdown,
  type IssueItem,
  useStoreSnapshot,
} from "@/lib/data";
import { useAgentboardState } from "@/lib/agentboard";
import { useNow, useNowInterval } from "@/lib/now";
import { invokeOrThrow } from "@/lib/tauri";
import { openExternalUrl } from "@/lib/open-url";
import { Empty, IssueRow, Panel, PrRow, prNeedsYou, prRank } from "@/components/store-bits";

/** A checkout the app already tracks (agentboard folder) that a Cockpit issue
 * can be dispatched into — its repo `origin` matches the issue's repo. */
type SlotTarget = { dir: string; branch: string; name: string };

/**
 * Does an agentboard repo's `origin` URL name the same GitHub repo as an issue's
 * `owner/name`? Folds the ssh/https/scp forms enough to compare the trailing
 * `owner/name` — the Rust guard (`validate_slot_for_repo`) re-checks
 * authoritatively before any dispatch, so this only needs to filter the menu.
 */
function repoMatches(originUrl: string | null | undefined, repo: string): boolean {
  if (!originUrl) return false;
  const norm = originUrl.toLowerCase().replace(/\.git$/, "").replace(/:/g, "/");
  return norm.endsWith(`/${repo.toLowerCase()}`);
}

/**
 * Cockpit — the day home. One dense screen: how long until the next meeting, the
 * PRs that need you, and the issue queue across repos. Read-only over the store
 * snapshot; the countdown is driven by the shared app clock.
 */
export function CockpitScreen() {
  const { snapshot, live } = useStoreSnapshot();
  const agentState = useAgentboardState();
  const now = useNow();

  // Candidate slot checkouts for an issue: the folders of every tracked repo
  // whose origin matches the issue's repo. Empty when none are tracked, which
  // disables the assign/branch actions with an explanatory item.
  const slotsFor = (repo: string): SlotTarget[] =>
    agentState.repos
      .filter((r) => repoMatches(r.originUrl, repo))
      .flatMap((r) => r.folders.map((f) => ({ dir: f.dir, branch: f.branch, name: f.name })));

  const nextEvent = currentOrNextEvent(snapshot.events, now);
  const meetingLive = nextEvent ? eventIsLive(nextEvent, now) : false;
  const msUntilStart = nextEvent && !meetingLive ? nextEvent.startTs - now : Infinity;
  const soon = nextEvent ? !meetingLive && nextEvent.startTs - now < 15 * 60_000 : false;
  // In the final approach the countdown shows m:ss — sharpen the shared clock to
  // 1s so it actually ticks second-by-second, then drop back once we pass it.
  useNowInterval(msUntilStart > 0 && msUntilStart < COUNTDOWN_SECONDS_THRESHOLD ? 1000 : undefined);
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
                .map((issue) => (
                  <IssueRow
                    key={`${issue.repo}#${issue.number}`}
                    issue={issue}
                    now={now}
                    actions={<IssueActions issue={issue} slots={slotsFor(issue.repo)} />}
                  />
                ))
            )}
          </Panel>
        </div>
      </ScrollArea>
    </div>
  );
}

/**
 * Per-issue action menu for the Cockpit issue queue: open the issue in the
 * browser, or dispatch it into a tracked slot checkout (assign via
 * `gh issue develop`, or just create a local branch from the issue title). The
 * slot submenus list the checkouts whose repo matches the issue; the Rust
 * command re-runs the clean-tree guard and reports success/failure via toast.
 */
function IssueActions({ issue, slots }: { issue: IssueItem; slots: SlotTarget[] }) {
  async function run(cmd: string, args: Record<string, unknown>) {
    try {
      const msg = await invokeOrThrow<string>(cmd, args);
      toast.success(msg);
    } catch (e) {
      toast.error(String(e));
    }
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          size="icon"
          variant="ghost"
          className="size-7 shrink-0 text-muted-foreground opacity-0 group-hover:opacity-100 data-[state=open]:opacity-100"
          aria-label="Issue actions"
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuItem onSelect={() => void openExternalUrl(issue.url)}>
          <ExternalLink className="size-4" />
          Open in browser
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <SlotSubmenu
          icon={<Send className="size-4" />}
          label="Assign to slot"
          slots={slots}
          onPick={(slot) =>
            void run("cockpit_assign_issue", {
              repo: issue.repo,
              number: issue.number,
              slotDir: slot.dir,
            })
          }
        />
        <SlotSubmenu
          icon={<GitBranchPlus className="size-4" />}
          label="Create branch"
          slots={slots}
          onPick={(slot) =>
            void run("cockpit_create_issue_branch", {
              repo: issue.repo,
              number: issue.number,
              title: issue.title,
              slotDir: slot.dir,
            })
          }
        />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** A submenu that lists candidate slot checkouts, or a disabled hint when the
 * issue's repo isn't tracked as an agentboard repo. */
function SlotSubmenu({
  icon,
  label,
  slots,
  onPick,
}: {
  icon: React.ReactNode;
  label: string;
  slots: SlotTarget[];
  onPick: (slot: SlotTarget) => void;
}) {
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        {icon}
        {label}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="w-64">
        {slots.length === 0 ? (
          <DropdownMenuItem disabled>No matching slot checkout</DropdownMenuItem>
        ) : (
          slots.map((slot) => (
            <DropdownMenuItem key={slot.dir} onSelect={() => onPick(slot)}>
              <div className="flex min-w-0 flex-col">
                <span className="truncate">{slot.name}</span>
                <span className="truncate font-mono text-xs text-muted-foreground">
                  {slot.branch}
                </span>
              </div>
            </DropdownMenuItem>
          ))
        )}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
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

