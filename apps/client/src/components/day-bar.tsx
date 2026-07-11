import { useEffect, useState } from "react";
import { CircleAlert, ListTodo } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useAgentboardState } from "@/lib/agentboard";
import { fmtAge, useStoreSnapshot } from "@/lib/data";
import { useWorkspace } from "@/lib/workspace";

/**
 * Persistent, quiet strip under the header: the top task and what needs you.
 * (The clock and next-meeting countdown live in the header's center cluster.)
 * Everything derived from `now` ticks every 30s.
 */
export function DayBar() {
  const { openTab } = useWorkspace();
  const { snapshot } = useStoreSnapshot();
  const agentState = useAgentboardState();
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(id);
  }, []);

  const topTask = snapshot.tasks
    .filter((t) => t.status !== "done")
    .sort((a, b) => a.createdAt - b.createdAt)[0];

  // Sessions blocked on you, summed across every repo's folders (`needs`).
  const waitingAgents = agentState.repos.reduce((sum, r) => sum + r.needs, 0);
  const failingPrs = snapshot.prs.filter(
    (p) => p.checks === "failing" || p.reviewState === "review_requested",
  ).length;
  const needsYou = waitingAgents + failingPrs;

  const claudeRuns = snapshot.runs.filter((r) => r.collector.startsWith("claude"));
  const newestRun = claudeRuns.reduce((max, r) => Math.max(max, r.ranAt), 0);
  const fresh = newestRun > 0 && now - newestRun < 30 * 60_000;

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

      <button
        className={cn(
          "flex items-center gap-1.5 rounded-md px-1.5 py-0.5 hover:bg-accent/50",
          needsYou === 0 && "text-muted-foreground/50",
          needsYou > 0 && "font-medium text-foreground",
        )}
        onClick={() => openTab("agentboard")}
      >
        {needsYou > 0 && <CircleAlert className="size-3.5 text-amber-500" />}
        {needsYou > 0 ? `${needsYou} need you` : "all clear"}
        {failingPrs > 0 && (
          <span className="text-red-500">· PR ✗</span>
        )}
      </button>

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
