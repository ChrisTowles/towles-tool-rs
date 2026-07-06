import { useMemo, useState } from "react";
import { ExternalLink, MoreHorizontal, Plus } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";
import {
  fmtClock,
  storeAddTask,
  storePromoteTaskToIssue,
  storeSetTaskStatus,
  TASK_STATUS_LABEL,
  TASK_STATUSES,
  useStoreSnapshot,
  type TaskItem,
  type TaskStatus,
} from "@/lib/data";
import { openExternalUrl } from "@/lib/open-url";

/**
 * Board — the cross-repo personal kanban over local todos. Columns are the five
 * task statuses; a card can be promoted to a real GitHub issue (optional link).
 * Read-only over the snapshot with local optimistic overlays for status moves
 * and freshly-added todos until the next `store://snapshot` arrives.
 */
export function BoardScreen() {
  const { snapshot } = useStoreSnapshot();
  const now = Date.now();

  const [statusOverrides, setStatusOverrides] = useState<Record<number, TaskStatus>>({});
  const [addedTasks, setAddedTasks] = useState<TaskItem[]>([]);
  const [draft, setDraft] = useState("");

  // Repos we know about (from collected PRs/issues + already-linked todos) — the
  // promote-to-issue targets.
  const repos = useMemo(() => {
    const set = new Set<string>();
    for (const p of snapshot.prs) set.add(p.repo);
    for (const i of snapshot.issues) set.add(i.repo);
    for (const t of snapshot.tasks) if (t.repo) set.add(t.repo);
    return [...set].sort();
  }, [snapshot.prs, snapshot.issues, snapshot.tasks]);

  const columns = useMemo(() => {
    const merged = [...addedTasks, ...snapshot.tasks].map((t) => ({
      ...t,
      status: statusOverrides[t.id] ?? t.status,
    }));
    const byStatus: Record<TaskStatus, TaskItem[]> = {
      backlog: [],
      next: [],
      doing: [],
      review: [],
      done: [],
    };
    for (const t of merged) byStatus[t.status]?.push(t);
    for (const s of TASK_STATUSES) {
      byStatus[s].sort((a, b) => a.position - b.position || a.createdAt - b.createdAt);
    }
    return byStatus;
  }, [snapshot.tasks, addedTasks, statusOverrides]);

  function move(id: number, status: TaskStatus) {
    setStatusOverrides((prev) => ({ ...prev, [id]: status }));
    void storeSetTaskStatus(id, status);
  }

  function promote(id: number, repo: string) {
    void storePromoteTaskToIssue(id, repo);
  }

  function addTask() {
    const text = draft.trim();
    if (!text) return;
    setAddedTasks((prev) => [
      { id: -Date.now(), text, status: "backlog", position: -1, createdAt: Date.now() },
      ...prev,
    ]);
    setDraft("");
    void storeAddTask(text);
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
        <h2 className="text-sm font-medium">Board</h2>
        <span className="text-xs text-muted-foreground">
          {repos.length} repos · {snapshot.tasks.length} todos
        </span>
        <div className="ml-auto flex items-center gap-1.5">
          <Input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addTask();
            }}
            placeholder="New todo…"
            className="h-7 w-56 text-sm"
          />
          <Button variant="ghost" size="icon-sm" aria-label="Add todo" onClick={addTask}>
            <Plus />
          </Button>
        </div>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="grid min-w-[900px] grid-cols-5 gap-3 p-3">
          {TASK_STATUSES.map((status) => (
            <div key={status} className="flex flex-col rounded-lg border bg-muted/30">
              <div className="flex items-center justify-between px-2.5 py-2">
                <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  {TASK_STATUS_LABEL[status]}
                </span>
                <span className="rounded-full bg-background px-1.5 font-mono text-[10px] text-muted-foreground">
                  {columns[status].length}
                </span>
              </div>
              <div className="flex flex-col gap-2 p-2 pt-0">
                {columns[status].map((task) => (
                  <Card
                    key={task.id}
                    task={task}
                    now={now}
                    repos={repos}
                    onMove={move}
                    onPromote={promote}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      </ScrollArea>
    </div>
  );
}

function Card({
  task,
  now,
  repos,
  onMove,
  onPromote,
}: {
  task: TaskItem;
  now: number;
  repos: string[];
  onMove: (id: number, status: TaskStatus) => void;
  onPromote: (id: number, repo: string) => void;
}) {
  const overdue = task.dueTs !== undefined && task.dueTs < now && task.status !== "done";
  return (
    <div
      className={cn(
        "rounded-md border bg-background p-2.5 text-sm shadow-sm",
        task.status === "done" && "opacity-60",
      )}
    >
      <div className="flex items-start gap-1.5">
        <span className={cn("min-w-0 flex-1", task.status === "done" && "line-through")}>
          {task.text}
        </span>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon-sm" className="-mr-1 -mt-1 size-6" aria-label="Todo actions">
              <MoreHorizontal className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-44">
            <DropdownMenuLabel>Move to</DropdownMenuLabel>
            {TASK_STATUSES.filter((s) => s !== task.status).map((s) => (
              <DropdownMenuItem key={s} onSelect={() => onMove(task.id, s)}>
                {TASK_STATUS_LABEL[s]}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
            {task.issueUrl ? (
              <DropdownMenuItem
                onSelect={() => task.issueUrl && void openExternalUrl(task.issueUrl)}
              >
                Open issue <ExternalLink className="ml-auto size-3.5" />
              </DropdownMenuItem>
            ) : repos.length > 0 ? (
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Create issue in…</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  {repos.map((repo) => (
                    <DropdownMenuItem key={repo} onSelect={() => onPromote(task.id, repo)}>
                      {repo}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            ) : (
              <DropdownMenuItem disabled>No repos to file in</DropdownMenuItem>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {(task.repo || task.dueTs !== undefined) && (
        <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
          {task.repo && task.issueNumber !== undefined && (
            <a
              href={task.issueUrl}
              target="_blank"
              rel="noreferrer"
              onClick={(e) => {
                e.preventDefault();
                if (task.issueUrl) void openExternalUrl(task.issueUrl);
              }}
              className="font-mono text-[11px] text-muted-foreground hover:text-foreground hover:underline"
            >
              {task.repo} #{task.issueNumber}
            </a>
          )}
          {task.dueTs !== undefined && (
            <Badge
              variant="outline"
              className={cn(
                "text-[10px]",
                overdue && "border-transparent bg-red-500/15 text-red-600 dark:text-red-400",
              )}
            >
              {fmtClock(task.dueTs)}
            </Badge>
          )}
        </div>
      )}
    </div>
  );
}
