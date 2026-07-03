import { CircleCheck, CircleDot, CircleX, Clock, GitMerge, GitPullRequestDraft } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { pullRequests, type PullRequest } from "@/lib/mock-data";

function StateIcon({ state }: { state: PullRequest["state"] }) {
  if (state === "merged") return <GitMerge className="size-4 shrink-0 text-purple-600 dark:text-purple-400" />;
  if (state === "draft") return <GitPullRequestDraft className="size-4 shrink-0 text-muted-foreground" />;
  return <CircleDot className="size-4 shrink-0 text-green-600 dark:text-green-500" />;
}

function ChecksIcon({ checks }: { checks: PullRequest["checks"] }) {
  if (checks === "passing") return <CircleCheck className="size-3.5 text-green-600 dark:text-green-500" />;
  if (checks === "failing") return <CircleX className="size-3.5 text-destructive" />;
  return <Clock className="size-3.5 text-amber-600 dark:text-amber-500" />;
}

export function GhPrsScreen() {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="font-heading text-lg font-semibold">Pull requests</h2>
        <p className="text-sm text-muted-foreground">ChrisTowles/towles-tool-rs</p>
      </div>

      <div className="divide-y rounded-lg border">
        {pullRequests.map((pr) => (
          <button
            key={pr.number}
            className="flex w-full items-center gap-3 px-3 py-2.5 text-left text-sm hover:bg-muted/50"
            onClick={() => toast.info("Opening PRs isn't wired to gh yet")}
          >
            <StateIcon state={pr.state} />
            <div className="flex-1 truncate">
              <div className="truncate">
                <span className="text-muted-foreground">#{pr.number}</span> {pr.title}
              </div>
              <div className="truncate font-mono text-xs text-muted-foreground">{pr.branch}</div>
            </div>
            <ChecksIcon checks={pr.checks} />
            <Badge variant={pr.state === "open" ? "default" : "secondary"}>{pr.state}</Badge>
            <span className="shrink-0 font-mono text-xs text-muted-foreground">{pr.updated}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
