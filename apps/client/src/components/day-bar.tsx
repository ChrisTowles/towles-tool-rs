import { useState } from "react";
import {
  CircleAlert,
  CircleX,
  GitPullRequest,
  ListTodo,
  MessageCircleHeart,
  type LucideIcon,
} from "lucide-react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useAgentboardState } from "@/lib/agentboard";
import { buildAttentionFeed, type AttentionItem, type AttentionKind } from "@/lib/attention-feed";
import { fmtAge, useStoreSnapshot } from "@/lib/data";
import { pickTopTask } from "@/lib/day-top-task";
import { useNow } from "@/lib/now";
import { openExternalUrl } from "@/lib/open-url";
import { useWorkspace } from "@/lib/workspace";

/**
 * Persistent, quiet strip under the header: the top task and what needs you.
 * (The clock and next-meeting countdown live in the header's center cluster.)
 * Everything derived from `now`, which comes from the shared app clock.
 */
export function DayBar() {
  const { openTab, openTabWithFocus } = useWorkspace();
  const { snapshot } = useStoreSnapshot();
  const agentState = useAgentboardState();
  const now = useNow();
  const [feedOpen, setFeedOpen] = useState(false);

  const topTask = pickTopTask(snapshot.tasks);

  // The attention feed is the single source for both the count and the popover
  // rows, so the badge number always equals the list length.
  const feed = buildAttentionFeed(snapshot, agentState);
  const needsYou = feed.length;

  const claudeRuns = snapshot.runs.filter((r) => r.collector.startsWith("claude"));
  const newestRun = claudeRuns.reduce((max, r) => Math.max(max, r.ranAt), 0);
  const fresh = newestRun > 0 && now - newestRun < 30 * 60_000;

  function navigate(item: AttentionItem) {
    setFeedOpen(false);
    if (item.url) {
      void openExternalUrl(item.url);
    } else if (item.target) {
      openTabWithFocus(item.target);
    }
  }

  return (
    <div className="flex h-8 shrink-0 items-center gap-2 border-b px-3 text-xs text-muted-foreground">
      {topTask && (
        <button
          className="flex items-center gap-1.5 rounded-md px-1.5 py-0.5 hover:bg-accent/50"
          onClick={() => openTab("cockpit")}
        >
          <ListTodo className="size-3.5" />
          <span className="max-w-56 truncate">{topTask.text}</span>
        </button>
      )}

      <div className="flex-1" />

      {needsYou === 0 ? (
        <span className="flex items-center gap-1.5 px-1.5 py-0.5 text-muted-foreground/50">
          all clear
        </span>
      ) : (
        <Popover open={feedOpen} onOpenChange={setFeedOpen}>
          <PopoverTrigger asChild>
            <button
              className="flex items-center gap-1.5 rounded-md px-1.5 py-0.5 font-medium text-foreground hover:bg-accent/50 data-[state=open]:bg-accent/50"
            >
              <CircleAlert className="size-3.5 text-amber-500" />
              {needsYou} need you
            </button>
          </PopoverTrigger>
          <PopoverContent align="end" className="w-80 gap-0 p-1.5">
            <div className="px-2 pb-1 pt-0.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
              Needs you
            </div>
            <div className="flex max-h-80 flex-col overflow-y-auto">
              {feed.map((item) => (
                <AttentionRow key={item.id} item={item} now={now} onNavigate={() => navigate(item)} />
              ))}
            </div>
          </PopoverContent>
        </Popover>
      )}

      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className={cn(
              "size-2 rounded-full",
              fresh ? "bg-green-500" : "bg-amber-500",
            )}
          />
        </TooltipTrigger>
        <TooltipContent>
          <div className="flex flex-col gap-0.5">
            {claudeRuns.length === 0 && <span>No collector runs yet</span>}
            {claudeRuns.map((r) => (
              <span key={r.collector}>
                {r.collector} · {fmtAge(r.ranAt, now)}
              </span>
            ))}
          </div>
        </TooltipContent>
      </Tooltip>
    </div>
  );
}

/** Icon + accent (paired dark variant) per attention kind. */
const KIND_META: Record<AttentionKind, { icon: LucideIcon; tone: string }> = {
  dm: { icon: MessageCircleHeart, tone: "text-rose-500 dark:text-rose-400" },
  "pr-ci": { icon: CircleX, tone: "text-red-500 dark:text-red-400" },
  "pr-review": { icon: GitPullRequest, tone: "text-blue-500 dark:text-blue-400" },
  agent: { icon: CircleAlert, tone: "text-amber-500 dark:text-amber-400" },
};

/** One feed row: navigates on click (external Slack link, or an in-app deep
 * link that scrolls+flashes the row on its screen). Reported, never actionable
 * — no approve/reply here. */
function AttentionRow({
  item,
  now,
  onNavigate,
}: {
  item: AttentionItem;
  now: number;
  onNavigate: () => void;
}) {
  const { icon: Icon, tone } = KIND_META[item.kind];
  return (
    <button
      onClick={onNavigate}
      className="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left hover:bg-accent/50"
    >
      <Icon className={cn("mt-0.5 size-3.5 shrink-0", tone)} />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-xs font-medium text-foreground">{item.title}</span>
        <span className="block truncate text-[11px] text-muted-foreground">{item.subtitle}</span>
      </span>
      {item.kind === "dm" && (
        <span className="mt-0.5 shrink-0 font-mono text-[10px] text-muted-foreground/60">
          {fmtAge(item.sortTs, now)}
        </span>
      )}
    </button>
  );
}
