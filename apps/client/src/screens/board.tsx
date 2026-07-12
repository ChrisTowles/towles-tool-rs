import { useMemo, useRef, useState } from "react";
import { CalendarPlus, ExternalLink, GripVertical, MoreHorizontal, Plus } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
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
  fmtDay,
  storeAddTask,
  storeDeleteTask,
  storePromoteTaskToIssue,
  storeSetTaskStatus,
  storeUpdateTask,
  TASK_STATUS_LABEL,
  TASK_STATUSES,
  useStoreSnapshot,
  type TaskItem,
  type TaskStatus,
} from "@/lib/data";
import { encodeTaskDrag, isTaskDrag, TASK_DRAG_TYPE, taskDropAction } from "@/lib/kanban-dnd";
import { openExternalUrl } from "@/lib/open-url";

/** Optimistic edits (text/due) applied over a snapshot todo until it re-arrives. */
type TaskEdit = { text?: string; dueTs?: number | undefined };

/** `YYYY-MM-DD` (local) for an `<input type="date">` value. */
function toDateInputValue(ms: number): string {
  const d = new Date(ms);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** Parse an `<input type="date">` value to epoch ms at the end of that local
 * day, so a due-today card is not overdue until the day actually ends. */
function dueDateToMs(value: string): number | undefined {
  const [y, m, d] = value.split("-").map(Number);
  if (!y || !m || !d) return undefined;
  return new Date(y, m - 1, d, 23, 59, 59, 999).getTime();
}

/**
 * Board — the cross-repo personal kanban over local todos. Columns are the five
 * task statuses; a card can be promoted to a real GitHub issue (optional link),
 * renamed, given/cleared a due date, or deleted. Read-only over the snapshot
 * with local optimistic overlays for status moves, edits, deletes, and
 * freshly-added todos until the next `store://snapshot` arrives.
 */
export function BoardScreen() {
  const { snapshot } = useStoreSnapshot();
  const now = Date.now();

  const [statusOverrides, setStatusOverrides] = useState<Record<number, TaskStatus>>({});
  const [editOverrides, setEditOverrides] = useState<Record<number, TaskEdit>>({});
  const [deletedIds, setDeletedIds] = useState<Set<number>>(() => new Set());
  const [addedTasks, setAddedTasks] = useState<TaskItem[]>([]);
  const [draft, setDraft] = useState("");
  // Column currently hovered by a card drag (drop-target highlight).
  const [dropTarget, setDropTarget] = useState<TaskStatus | null>(null);

  // Repos we know about (from collected PRs/issues + already-linked todos) — the
  // promote-to-issue targets.
  const repos = useMemo(() => {
    const set = new Set<string>();
    for (const p of snapshot.prs) set.add(p.repo);
    for (const i of snapshot.issues) set.add(i.repo);
    for (const t of snapshot.tasks) if (t.repo) set.add(t.repo);
    return [...set].sort();
  }, [snapshot.prs, snapshot.issues, snapshot.tasks]);

  const merged = useMemo(
    () =>
      [...addedTasks, ...snapshot.tasks]
        .filter((t) => !deletedIds.has(t.id))
        .map((t) => ({
          ...t,
          ...editOverrides[t.id],
          status: statusOverrides[t.id] ?? t.status,
        })),
    [snapshot.tasks, addedTasks, editOverrides, statusOverrides, deletedIds],
  );

  const columns = useMemo(() => {
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
  }, [merged]);

  function move(id: number, status: TaskStatus) {
    setStatusOverrides((prev) => ({ ...prev, [id]: status }));
    void storeSetTaskStatus(id, status);
  }

  function promote(id: number, repo: string) {
    void storePromoteTaskToIssue(id, repo);
  }

  // Rename and due-date edits both re-send the todo's other free-form fields
  // (`storeUpdateTask` is a full replace of text/notes/due), reading them from
  // `merged` so chained optimistic edits compose.
  function rename(id: number, text: string) {
    const trimmed = text.trim();
    const current = merged.find((t) => t.id === id);
    if (!current || !trimmed || trimmed === current.text) return;
    setEditOverrides((prev) => ({ ...prev, [id]: { ...prev[id], text: trimmed } }));
    void storeUpdateTask(id, trimmed, current.notes, current.dueTs);
  }

  function setDue(id: number, dueTs: number | undefined) {
    const current = merged.find((t) => t.id === id);
    if (!current) return;
    setEditOverrides((prev) => ({ ...prev, [id]: { ...prev[id], dueTs } }));
    void storeUpdateTask(id, current.text, current.notes, dueTs);
  }

  function remove(id: number) {
    setDeletedIds((prev) => new Set(prev).add(id));
    void storeDeleteTask(id);
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
            <div
              key={status}
              onDragOver={(e) => {
                if (!isTaskDrag(e.dataTransfer.types)) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                setDropTarget(status);
              }}
              onDragLeave={(e) => {
                // Ignore moves between children of the same column.
                if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
                setDropTarget((cur) => (cur === status ? null : cur));
              }}
              onDrop={(e) => {
                e.preventDefault();
                setDropTarget(null);
                const action = taskDropAction(e.dataTransfer.getData(TASK_DRAG_TYPE), status);
                if (action) move(action.id, action.status);
              }}
              className={cn(
                "flex flex-col rounded-lg border bg-muted/30",
                dropTarget === status &&
                  "border-violet-500/60 bg-violet-500/5 dark:bg-violet-500/10",
              )}
            >
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
                    onRename={rename}
                    onSetDue={setDue}
                    onDelete={remove}
                    onDragEnd={() => setDropTarget(null)}
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
  onRename,
  onSetDue,
  onDelete,
  onDragEnd,
}: {
  task: TaskItem;
  now: number;
  repos: string[];
  onMove: (id: number, status: TaskStatus) => void;
  onPromote: (id: number, repo: string) => void;
  onRename: (id: number, text: string) => void;
  onSetDue: (id: number, dueTs: number | undefined) => void;
  onDelete: (id: number) => void;
  onDragEnd: () => void;
}) {
  const [dragging, setDragging] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState(task.text);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const overdue = task.dueTs !== undefined && task.dueTs < now && task.status !== "done";

  function startRename() {
    setEditValue(task.text);
    setEditing(true);
  }

  function commitRename() {
    setEditing(false);
    onRename(task.id, editValue);
  }

  return (
    <div
      draggable={!editing}
      onDragStart={(e) => {
        e.dataTransfer.setData(
          TASK_DRAG_TYPE,
          encodeTaskDrag({ id: task.id, status: task.status }),
        );
        e.dataTransfer.effectAllowed = "move";
        setDragging(true);
      }}
      onDragEnd={() => {
        setDragging(false);
        onDragEnd();
      }}
      className={cn(
        "group rounded-md border bg-background p-2.5 text-sm shadow-sm",
        "cursor-grab active:cursor-grabbing",
        task.status === "done" && "opacity-60",
        dragging && "opacity-40",
      )}
    >
      <div className="flex items-start gap-1.5">
        <GripVertical
          aria-hidden
          className="-ml-1 mt-0.5 size-3.5 shrink-0 text-muted-foreground/40 opacity-0 transition-opacity group-hover:opacity-100"
        />
        {editing ? (
          <Input
            ref={inputRef}
            autoFocus
            value={editValue}
            onChange={(e) => setEditValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitRename();
              if (e.key === "Escape") setEditing(false);
            }}
            onBlur={commitRename}
            className="h-6 min-w-0 flex-1 px-1.5 py-0 text-sm"
            aria-label="Rename todo"
          />
        ) : (
          <span
            onDoubleClick={startRename}
            className={cn("min-w-0 flex-1", task.status === "done" && "line-through")}
          >
            {task.text}
          </span>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon-sm"
              className="-mr-1 -mt-1 size-6"
              aria-label="Todo actions"
            >
              <MoreHorizontal className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onSelect={startRename}>Rename</DropdownMenuItem>
            <DropdownMenuLabel className="pb-0.5 pt-1 text-muted-foreground">
              Due date
            </DropdownMenuLabel>
            <div
              className="flex items-center gap-1 px-2 py-1"
              // Keep the menu open and stop its typeahead from eating keystrokes.
              onKeyDown={(e) => e.stopPropagation()}
            >
              <Input
                type="date"
                value={task.dueTs !== undefined ? toDateInputValue(task.dueTs) : ""}
                onChange={(e) =>
                  onSetDue(task.id, e.target.value ? dueDateToMs(e.target.value) : undefined)
                }
                className="h-7 flex-1 px-1.5 text-xs"
                aria-label="Set due date"
              />
              {task.dueTs !== undefined && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  onClick={() => onSetDue(task.id, undefined)}
                >
                  Clear
                </Button>
              )}
            </div>
            <DropdownMenuSeparator />
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
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant="destructive"
              onSelect={() => setConfirmingDelete(true)}
            >
              Delete
            </DropdownMenuItem>
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
                "gap-1 text-[10px]",
                overdue && "border-transparent bg-red-500/15 text-red-600 dark:text-red-400",
              )}
            >
              <CalendarPlus aria-hidden className="size-3" />
              {fmtDay(task.dueTs)}
            </Badge>
          )}
        </div>
      )}

      <AlertDialog open={confirmingDelete} onOpenChange={setConfirmingDelete}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this todo?</AlertDialogTitle>
            <AlertDialogDescription>
              “{task.text}” will be permanently removed. This can't be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => onDelete(task.id)}
              className="bg-red-600 text-white hover:bg-red-600/90"
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
