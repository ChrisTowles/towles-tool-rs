import { useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarClock,
  CircleAlert,
  CircleDot,
  ExternalLink,
  GitBranch,
  GitBranchPlus,
  GitPullRequest,
  Link as LinkIcon,
  ListChecks,
  MoreHorizontal,
  RefreshCw,
  Send,
  Video,
} from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
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
  fmtAge,
  fmtClock,
  fmtCountdown,
  type IssueItem,
  type PrItem,
  storeCollectNow,
  useStoreSnapshot,
} from "@/lib/data";
import { cockpitRepos, filterByRepo } from "@/lib/cockpit-filter";
import { dataRefreshedAt } from "@/lib/collector-health";
import { useAgentboardState } from "@/lib/agentboard";
import { useNow, useNowInterval } from "@/lib/now";
import { invokeOrThrow } from "@/lib/tauri";
import { openExternalUrl } from "@/lib/open-url";
import {
  Empty,
  IssueRow,
  Panel,
  PrRow,
  prNeedsYou,
  prRank,
} from "@/components/store-bits";

/** A checkout the app already tracks (agentboard folder) that a Cockpit issue
 * can be dispatched into — its repo `origin` matches the issue's repo. */
type SlotTarget = { dir: string; branch: string; name: string };

/**
 * Does an agentboard repo's `origin` URL name the same GitHub repo as an issue's
 * `owner/name`? Folds the ssh/https/scp forms enough to compare the trailing
 * `owner/name` — the Rust guard (`validate_slot_for_repo`) re-checks
 * authoritatively before any dispatch, so this only needs to filter the menu.
 */
function repoMatches(
  originUrl: string | null | undefined,
  repo: string,
): boolean {
  if (!originUrl) return false;
  const norm = originUrl
    .toLowerCase()
    .replace(/\.git$/, "")
    .replace(/:/g, "/");
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

  // How stale the PR/issue panels are, and a way to force a refresh. The button
  // disables while a run is in flight; it clears once a newer refresh-collector
  // run lands (the store re-emits its snapshot) or after a safety timeout, so it
  // never sticks disabled.
  const refreshedAt = dataRefreshedAt(snapshot.runs, now);
  const [refreshing, setRefreshing] = useState(false);
  const refreshBaseline = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (!refreshing) return;
    const landed =
      refreshedAt !== undefined &&
      (refreshBaseline.current === undefined ||
        refreshedAt > refreshBaseline.current);
    if (landed) {
      setRefreshing(false);
      return;
    }
    const t = setTimeout(() => setRefreshing(false), 30_000);
    return () => clearTimeout(t);
  }, [refreshing, refreshedAt]);

  async function refresh() {
    refreshBaseline.current = refreshedAt;
    setRefreshing(true);
    // `false` = overlap (a run was already in flight) or browser dev — nothing
    // new to wait on, so drop straight back to idle.
    if (!(await storeCollectNow())) setRefreshing(false);
  }

  // Candidate slot checkouts for an issue: the folders of every tracked repo
  // whose origin matches the issue's repo. Empty when none are tracked, which
  // disables the assign/branch actions with an explanatory item.
  const slotsFor = (repo: string): SlotTarget[] =>
    agentState.repos
      .filter((r) => repoMatches(r.originUrl, repo))
      .flatMap((r) =>
        r.folders.map((f) => ({ dir: f.dir, branch: f.branch, name: f.name })),
      );

  const nextEvent = currentOrNextEvent(snapshot.events, now);
  const meetingLive = nextEvent ? eventIsLive(nextEvent, now) : false;
  const msUntilStart =
    nextEvent && !meetingLive ? nextEvent.startTs - now : Infinity;
  const soon = nextEvent
    ? !meetingLive && nextEvent.startTs - now < 15 * 60_000
    : false;
  // In the final approach the countdown shows m:ss — sharpen the shared clock to
  // 1s so it actually ticks second-by-second, then drop back once we pass it.
  useNowInterval(
    msUntilStart > 0 && msUntilStart < COUNTDOWN_SECONDS_THRESHOLD
      ? 1000
      : undefined,
  );
  // Highlight amber while a meeting is live or imminent.
  const highlight = meetingLive || soon;
  const later = snapshot.events
    .filter((e) => e.startTs > now && e.id !== nextEvent?.id)
    .sort((a, b) => a.startTs - b.startTs)
    .slice(0, 2);

  // Repo filter chips: narrow both panels to one repo when zoning in. The strip
  // gauges stay whole-snapshot totals (the overview); the chips + panels + their
  // note counts move together. A selection that no longer exists (its repo got
  // collected away) falls back to "all" without a stale-state effect.
  const [repoFilter, setRepoFilter] = useState<string | null>(null);
  // Merged PRs live in the snapshot too (briefly, so a folder's rail chip can
  // turn purple), but Cockpit's PR queue is open work — exclude them.
  const openPrs = useMemo(() => snapshot.prs.filter((p) => p.state === "open"), [snapshot.prs]);
  const repoList = cockpitRepos(openPrs, snapshot.issues);
  const activeRepo =
    repoFilter !== null && repoList.includes(repoFilter) ? repoFilter : null;
  const visiblePrs = filterByRepo(openPrs, activeRepo);
  const visibleIssues = filterByRepo(snapshot.issues, activeRepo);

  const needsYouPrs = openPrs.filter(prNeedsYou);
  const visibleNeedsYou = visiblePrs.filter(prNeedsYou);

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Next-meeting strip */}
      <div className="flex shrink-0 flex-wrap items-center gap-x-8 gap-y-2 border-b px-5 py-4">
        <div className="flex items-center gap-3">
          <CalendarClock
            className={cn(
              "size-5",
              highlight ? "text-amber-500" : "text-muted-foreground",
            )}
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
                    if (nextEvent.joinUrl)
                      void openExternalUrl(nextEvent.joinUrl);
                  }}
                >
                  <Video />
                  Join
                </Button>
              ) : null}
            </div>
          ) : (
            <span className="text-sm text-muted-foreground">
              No more meetings today.
            </span>
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
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => void refresh()}
                disabled={refreshing}
                className="flex items-center gap-1.5 rounded-md px-1.5 py-1 text-muted-foreground hover:bg-accent/50 disabled:pointer-events-none disabled:opacity-60"
                aria-label="Refresh PRs and issues"
              >
                <RefreshCw
                  className={cn("size-3.5", refreshing && "animate-spin")}
                />
                <span className="tabular-nums">
                  {refreshing
                    ? "Refreshing…"
                    : refreshedAt !== undefined
                      ? `Refreshed ${fmtAge(refreshedAt, now)}`
                      : "Refresh"}
                </span>
              </button>
            </TooltipTrigger>
            <TooltipContent>
              Refresh pull requests and issues now
            </TooltipContent>
          </Tooltip>
          <Gauge
            n={needsYouPrs.length}
            label="PRs need you"
            tone={needsYouPrs.length ? "warn" : "muted"}
          />
          <Gauge n={snapshot.issues.length} label="Issues" tone="muted" />
          <Gauge n={repoList.length} label="Repos" tone="muted" />
        </div>
      </div>

      {/* Repo filter chips — narrow both panels to one repo (only worth showing
          when there's more than one to choose between). */}
      {repoList.length > 1 && (
        <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b px-5 py-2">
          <RepoChip
            label="All repos"
            active={activeRepo === null}
            onClick={() => setRepoFilter(null)}
          />
          {repoList.map((repo) => (
            <RepoChip
              key={repo}
              label={repo}
              active={activeRepo === repo}
              onClick={() =>
                setRepoFilter((cur) => (cur === repo ? null : repo))
              }
            />
          ))}
        </div>
      )}

      {!live && (
        <div className="flex shrink-0 items-center gap-2 border-b bg-amber-500/10 px-5 py-1.5 text-xs text-amber-700 dark:text-amber-400">
          <CircleAlert className="size-3.5 shrink-0" />
          Not connected to the store — open this window in the Towles Tool app
          to see live PRs, issues, and events.
        </div>
      )}

      <ScrollArea className="min-h-0 flex-1">
        <div className="grid grid-cols-1 gap-4 p-4 lg:grid-cols-2">
          {/* Pull requests */}
          <Panel
            title="Pull requests"
            note={`${visibleNeedsYou.length} need you`}
            icon={<GitPullRequest className="size-4" />}
          >
            {visiblePrs.length === 0 ? (
              <Empty>
                {live ? "No open PRs across your repos." : "Not connected yet."}
              </Empty>
            ) : (
              visiblePrs
                .slice()
                .sort(
                  (a, b) => prRank(b) - prRank(a) || b.updatedTs - a.updatedTs,
                )
                .map((pr) => (
                  <PrRow
                    key={`${pr.repo}#${pr.number}`}
                    pr={pr}
                    now={now}
                    actions={<PrActions pr={pr} />}
                  />
                ))
            )}
          </Panel>

          {/* Issue queue */}
          <Panel
            title="Issue queue"
            note={`${visibleIssues.length} open`}
            icon={<CircleDot className="size-4" />}
          >
            {visibleIssues.length === 0 ? (
              <Empty>
                {live ? "No issues assigned to you." : "Not connected yet."}
              </Empty>
            ) : (
              visibleIssues
                .slice()
                .sort((a, b) => b.updatedTs - a.updatedTs)
                .map((issue) => (
                  <IssueRow
                    key={`${issue.repo}#${issue.number}`}
                    issue={issue}
                    now={now}
                    actions={
                      <IssueActions
                        issue={issue}
                        slots={slotsFor(issue.repo)}
                      />
                    }
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
function IssueActions({
  issue,
  slots,
}: {
  issue: IssueItem;
  slots: SlotTarget[];
}) {
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

/**
 * Per-PR action menu for the Cockpit pull-requests panel. Navigation and
 * clipboard only — open the PR (or its checks tab) in the browser, copy the
 * branch name or PR URL. No merge/review/approve actions: PR state is reported
 * here, never re-rendered or acted on (that happens on GitHub).
 */
function PrActions({ pr }: { pr: PrItem }) {
  async function copy(text: string, what: string) {
    try {
      await navigator.clipboard.writeText(text);
      toast.success(`Copied ${what}`);
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
          aria-label="PR actions"
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuItem onSelect={() => void openExternalUrl(pr.url)}>
          <ExternalLink className="size-4" />
          Open in browser
        </DropdownMenuItem>
        <DropdownMenuItem
          onSelect={() => void openExternalUrl(`${pr.url}/checks`)}
        >
          <ListChecks className="size-4" />
          Open checks
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void copy(pr.branch, "branch name")}>
          <GitBranch className="size-4" />
          Copy branch name
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => void copy(pr.url, "PR URL")}>
          <LinkIcon className="size-4" />
          Copy PR URL
        </DropdownMenuItem>
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
          <DropdownMenuItem disabled>
            No matching slot checkout
          </DropdownMenuItem>
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

/** A repo filter chip under the strip. Violet marks the active selection (the
 * "currently focused" accent); the rest are neutral until hovered. */
function RepoChip({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "rounded-md border px-2 py-0.5 font-mono text-[11px] transition-colors",
        active
          ? "border-violet-500/60 bg-violet-500/10 text-foreground"
          : "border-transparent bg-muted text-muted-foreground hover:bg-accent",
      )}
    >
      {label}
    </button>
  );
}

function Gauge({
  n,
  label,
  tone,
}: {
  n: number;
  label: string;
  tone: "warn" | "muted";
}) {
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
      <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
        {label}
      </span>
    </div>
  );
}
