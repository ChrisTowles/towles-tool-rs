import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import {
  CalendarPlus,
  ExternalLink,
  GripVertical,
  ListTodo,
  MoreHorizontal,
  Plus,
  Search,
  StickyNote,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Textarea } from "@/components/ui/textarea";
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
  storeClearDone,
  storeDeleteTask,
  storePromoteTaskToIssue,
  storeSetTaskPosition,
  storeSetTaskStatus,
  storeUpdateTask,
  TASK_STATUS_LABEL,
  TASK_STATUSES,
  useStoreSnapshot,
  type TaskItem,
  type TaskStatus,
} from "@/lib/data";
import {
  decodeTaskDrag,
  encodeTaskDrag,
  isTaskDrag,
  reorderedPosition,
  TASK_DRAG_TYPE,
} from "@/lib/kanban-dnd";
import { dueState, overdueByStatus } from "@/lib/board-metrics";
import { matchesTaskFilter } from "@/lib/board-filter";
import { parseQuickAdd } from "@/lib/quick-add";
import { useFocusTarget } from "@/lib/focus-target";
import { useNow } from "@/lib/now";
import { openExternalUrl } from "@/lib/open-url";

/** Optimistic edits (text/notes/due) applied over a snapshot todo until it
 * re-arrives. */
type TaskEdit = { text?: string; notes?: string | undefined; dueTs?: number | undefined };

/** Optimistic status + fractional position from a drag-reorder, until re-arrival. */
type PosOverride = { status: TaskStatus; position: number };

/** The slot a card would drop into: before `beforeId`, or at a column's end. */
type DropSlot = { status: TaskStatus; beforeId: number | "end" };

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
  const now = useNow();
  // Deep-link focus: a promoted-todo / board deep link scrolls the card here.
  const focusRef = useFocusTarget<HTMLDivElement>("board");

  const [statusOverrides, setStatusOverrides] = useState<Record<number, TaskStatus>>({});
  const [posOverrides, setPosOverrides] = useState<Record<number, PosOverride>>({});
  const [editOverrides, setEditOverrides] = useState<Record<number, TaskEdit>>({});
  const [deletedIds, setDeletedIds] = useState<Set<number>>(() => new Set());
  const [addedTasks, setAddedTasks] = useState<TaskItem[]>([]);
  const [draft, setDraft] = useState("");
  // Quick filter: case-insensitive substring over each todo's text + repo tag.
  const [filter, setFilter] = useState("");
  // The insertion slot the current drag would land in: drives both the column
  // highlight (`dropSlot.status`) and the drop line before `beforeId`.
  const [dropSlot, setDropSlot] = useState<DropSlot | null>(null);

  // Repos we know about (from collected PRs/issues + already-linked todos) — the
  // promote-to-issue targets.
  const repos = useMemo(() => {
    const set = new Set<string>();
    for (const p of snapshot.prs) set.add(p.repo);
    for (const i of snapshot.issues) set.add(i.repo);
    for (const t of snapshot.tasks) if (t.repo) set.add(t.repo);
    return [...set].sort();
  }, [snapshot.prs, snapshot.issues, snapshot.tasks]);

  // Drop an optimistic quick-add copy once the store snapshot delivers the real
  // row — matched on the content the add sent, since the store assigns the id.
  useEffect(() => {
    setAddedTasks((prev) => {
      if (prev.length === 0) return prev;
      const next = prev.filter(
        (a) =>
          !snapshot.tasks.some(
            (t) => t.text === a.text && t.repo === a.repo && t.dueTs === a.dueTs,
          ),
      );
      return next.length === prev.length ? prev : next;
    });
  }, [snapshot.tasks]);

  const merged = useMemo(
    () =>
      [...addedTasks, ...snapshot.tasks]
        .filter((t) => !deletedIds.has(t.id))
        .map((t) => {
          const pos = posOverrides[t.id];
          return {
            ...t,
            ...editOverrides[t.id],
            // A reorder override carries both the target column and a fractional
            // position; it wins over a plain status move for the same card.
            status: pos?.status ?? statusOverrides[t.id] ?? t.status,
            position: pos ? pos.position : t.position,
          };
        }),
    [snapshot.tasks, addedTasks, editOverrides, statusOverrides, posOverrides, deletedIds],
  );

  // The cards actually rendered: everything matching the quick filter (an empty
  // filter matches all). `n hidden` below is the count the filter removes.
  const visible = useMemo(
    () => merged.filter((t) => matchesTaskFilter(t, filter)),
    [merged, filter],
  );
  const hiddenCount = merged.length - visible.length;
  // Truly empty: no todos in any column (a filter hiding all is a different
  // state — the header still shows the count and the filter box).
  const isEmpty = merged.length === 0;

  const columns = useMemo(() => {
    const byStatus: Record<TaskStatus, TaskItem[]> = {
      backlog: [],
      next: [],
      doing: [],
      review: [],
      done: [],
    };
    for (const t of visible) byStatus[t.status]?.push(t);
    for (const s of TASK_STATUSES) {
      byStatus[s].sort((a, b) => a.position - b.position || a.createdAt - b.createdAt);
    }
    return byStatus;
  }, [visible]);

  // Overdue cards per column, for the header's red load pip.
  const overdue = useMemo(() => overdueByStatus(visible, now), [visible, now]);

  function move(id: number, status: TaskStatus) {
    setStatusOverrides((prev) => ({ ...prev, [id]: status }));
    // A plain column move appends; drop any stale reorder slot for this card.
    setPosOverrides((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
    void storeSetTaskStatus(id, status);
  }

  // Reorder `id` into `status` just before `beforeId` ("end" = append). Computes
  // a fractional optimistic position from the neighbors so the card sorts into
  // its new slot immediately, and sends the integer slot index to the backend
  // (which renumbers the column). The backend orders by real positions, so the
  // insertion index is taken from the currently-displayed column order.
  function reorder(id: number, status: TaskStatus, beforeId: number | "end") {
    if (beforeId === id) return;
    const col = columns[status].filter((t) => t.id !== id);
    const insertAt =
      beforeId === "end" ? col.length : Math.max(0, col.findIndex((t) => t.id === beforeId));
    const prev = col[insertAt - 1] ?? null;
    const next = col[insertAt] ?? null;
    const position = reorderedPosition(prev ? prev.position : null, next ? next.position : null);
    setPosOverrides((p) => ({ ...p, [id]: { status, position } }));
    // The reorder now owns this card's column; drop any plain status override.
    setStatusOverrides((p) => {
      if (!(id in p)) return p;
      const nextOv = { ...p };
      delete nextOv[id];
      return nextOv;
    });
    void storeSetTaskPosition(id, status, insertAt);
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

  function setNotes(id: number, notes: string) {
    const current = merged.find((t) => t.id === id);
    if (!current) return;
    // Empty/whitespace-only notes clear the field back to unset.
    const value = notes.trim() === "" ? undefined : notes;
    setEditOverrides((prev) => ({ ...prev, [id]: { ...prev[id], notes: value } }));
    void storeUpdateTask(id, current.text, value, current.dueTs);
  }

  function remove(id: number) {
    setDeletedIds((prev) => new Set(prev).add(id));
    void storeDeleteTask(id);
  }

  function clearDone() {
    void storeClearDone();
  }

  // Live preview of the quick-add tokens (`@today`/`@tomorrow`/`@YYYY-MM-DD` due
  // dates, `#owner/repo` repo tag) parsed out of the New-todo draft, so the hint
  // under the input shows what will be stored before the todo is added.
  const parsedDraft = useMemo(() => parseQuickAdd(draft, now), [draft, now]);

  function addTask() {
    const { text, dueTs, repo } = parsedDraft;
    if (!text) return;
    setAddedTasks((prev) => [
      {
        id: -Date.now(),
        text,
        status: "backlog",
        position: -1,
        dueTs,
        repo,
        createdAt: Date.now(),
      },
      ...prev,
    ]);
    setDraft("");
    void storeAddTask(text, dueTs, repo);
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
        <h2 className="text-sm font-medium">Board</h2>
        <span className="text-xs text-muted-foreground">
          {repos.length} repos · {snapshot.tasks.length} todos
        </span>
        {filter.trim() !== "" && hiddenCount > 0 && (
          <span className="text-xs text-muted-foreground">{hiddenCount} hidden</span>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          <div className="relative w-44">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") setFilter("");
              }}
              placeholder="Filter…"
              className="h-7 pl-7 text-sm"
              spellCheck={false}
              aria-label="Filter todos"
            />
          </div>
          <div className="relative w-56">
            <Input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") addTask();
              }}
              placeholder="New todo… @tomorrow #owner/repo"
              className="h-7 w-full text-sm"
            />
            {(parsedDraft.dueTs !== undefined || parsedDraft.repo) && (
              <div className="absolute top-full left-0 z-10 mt-1 flex items-center gap-1.5 whitespace-nowrap text-[11px] text-muted-foreground">
                {parsedDraft.dueTs !== undefined && (
                  <span className="flex items-center gap-1">
                    <CalendarPlus aria-hidden className="size-3" />
                    {fmtDay(parsedDraft.dueTs)}
                  </span>
                )}
                {parsedDraft.repo && (
                  <span className="font-mono">#{parsedDraft.repo}</span>
                )}
              </div>
            )}
          </div>
          <Button variant="ghost" size="icon-sm" aria-label="Add todo" onClick={addTask}>
            <Plus />
          </Button>
        </div>
      </div>

      {isEmpty ? (
        <div ref={focusRef} className="flex min-h-0 flex-1 items-center justify-center p-6">
          <div className="flex max-w-sm flex-col items-center gap-2 text-center">
            <ListTodo aria-hidden className="size-8 text-muted-foreground/50" />
            <p className="text-sm font-medium">No tasks yet</p>
            <p className="text-xs text-muted-foreground">
              Add one with the <span className="font-medium text-foreground">New todo…</span> box up
              top — tag a due date with <span className="font-mono">@tomorrow</span> and a repo with{" "}
              <span className="font-mono">#owner/repo</span>.
            </p>
          </div>
        </div>
      ) : (
        <ScrollArea className="min-h-0 flex-1">
          <div ref={focusRef} className="grid min-w-[900px] grid-cols-5 gap-3 p-3">
          {TASK_STATUSES.map((status) => (
            <div
              key={status}
              onDragOver={(e) => {
                if (!isTaskDrag(e.dataTransfer.types)) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                // Over the column's empty tail (cards handle their own hover and
                // stop propagation), so the card would land at the end.
                setDropSlot({ status, beforeId: "end" });
              }}
              onDragLeave={(e) => {
                // Ignore moves between children of the same column.
                if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
                setDropSlot((cur) => (cur?.status === status ? null : cur));
              }}
              onDrop={(e) => {
                e.preventDefault();
                setDropSlot(null);
                const payload = decodeTaskDrag(e.dataTransfer.getData(TASK_DRAG_TYPE));
                if (payload) reorder(payload.id, status, "end");
              }}
              className={cn(
                "flex flex-col rounded-lg border bg-muted/30",
                dropSlot?.status === status &&
                  "border-violet-500/60 bg-violet-500/5 dark:bg-violet-500/10",
              )}
            >
              <div className="flex items-center justify-between px-2.5 py-2">
                <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
                  {TASK_STATUS_LABEL[status]}
                </span>
                <span className="flex items-center gap-1">
                  {status === "done" && columns.done.length > 0 && (
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-5 px-1.5 text-[10px] uppercase tracking-wide text-muted-foreground"
                      title="Remove done todos completed over 7 days ago"
                      onClick={clearDone}
                    >
                      Clear done
                    </Button>
                  )}
                  {overdue[status] > 0 && (
                    <span
                      title={`${overdue[status]} overdue`}
                      className="rounded-full border border-transparent bg-red-500/15 px-1.5 font-mono text-[10px] text-red-600 dark:text-red-400"
                    >
                      {overdue[status]} late
                    </span>
                  )}
                  <span className="rounded-full bg-background px-1.5 font-mono text-[10px] text-muted-foreground">
                    {columns[status].length}
                  </span>
                </span>
              </div>
              <div className="flex flex-col gap-2 p-2 pt-0">
                {columns[status].map((task, i) => (
                  <Fragment key={task.id}>
                    <DropLine
                      active={dropSlot?.status === status && dropSlot.beforeId === task.id}
                    />
                    <Card
                      task={task}
                      now={now}
                      repos={repos}
                      nextId={columns[status][i + 1]?.id ?? null}
                      onMove={move}
                      onReorderHover={setDropSlot}
                      onReorder={reorder}
                      onPromote={promote}
                      onRename={rename}
                      onSetDue={setDue}
                      onSetNotes={setNotes}
                      onDelete={remove}
                      onDragEnd={() => setDropSlot(null)}
                    />
                  </Fragment>
                ))}
                <DropLine active={dropSlot?.status === status && dropSlot.beforeId === "end"} />
              </div>
            </div>
          ))}
          </div>
        </ScrollArea>
      )}
    </div>
  );
}

/**
 * The notes editor inside a card's dropdown. Seeded from the todo's current
 * notes each time the menu opens (Radix unmounts the content on close), edited
 * locally, and committed on blur so we don't write to the store per keystroke.
 */
function NotesField({
  task,
  onSetNotes,
}: {
  task: TaskItem;
  onSetNotes: (id: number, notes: string) => void;
}) {
  const [draft, setDraft] = useState(task.notes ?? "");
  return (
    <Textarea
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => {
        if (draft.trim() !== (task.notes ?? "").trim()) onSetNotes(task.id, draft);
      }}
      placeholder="Add notes…"
      rows={3}
      className="min-h-16 resize-none text-xs"
      aria-label="Todo notes"
    />
  );
}

/** The insertion indicator drawn between cards at the current drop slot. */
function DropLine({ active }: { active: boolean }) {
  if (!active) return null;
  return <div aria-hidden className="h-0.5 rounded-full bg-violet-500" />;
}

function Card({
  task,
  now,
  repos,
  nextId,
  onMove,
  onReorderHover,
  onReorder,
  onPromote,
  onRename,
  onSetDue,
  onSetNotes,
  onDelete,
  onDragEnd,
}: {
  task: TaskItem;
  now: number;
  repos: string[];
  /** The card below this one in the column (for a bottom-half drop), or null. */
  nextId: number | null;
  onMove: (id: number, status: TaskStatus) => void;
  onReorderHover: (slot: DropSlot) => void;
  onReorder: (id: number, status: TaskStatus, beforeId: number | "end") => void;
  onPromote: (id: number, repo: string) => void;
  onRename: (id: number, text: string) => void;
  onSetDue: (id: number, dueTs: number | undefined) => void;
  onSetNotes: (id: number, notes: string) => void;
  onDelete: (id: number) => void;
  onDragEnd: () => void;
}) {
  const [dragging, setDragging] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState(task.text);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  // A shipped card is never "late", so done cards carry no due accent.
  const due = task.status === "done" ? "none" : dueState(task.dueTs, now);
  const hasNotes = (task.notes ?? "").trim() !== "";

  function startRename() {
    setEditValue(task.text);
    setEditing(true);
  }

  function commitRename() {
    setEditing(false);
    onRename(task.id, editValue);
  }

  // The insertion slot for a drag hovering this card: before it (pointer in the
  // top half) or before the card below it (bottom half; "end" if it's last).
  function slotBeforeId(e: React.DragEvent<HTMLDivElement>): number | "end" {
    const rect = e.currentTarget.getBoundingClientRect();
    const inLowerHalf = e.clientY > rect.top + rect.height / 2;
    return inLowerHalf ? (nextId ?? "end") : task.id;
  }

  return (
    <div
      draggable={!editing}
      data-focus-kind="todo"
      data-focus-id={String(task.id)}
      onDragStart={(e) => {
        e.dataTransfer.setData(
          TASK_DRAG_TYPE,
          encodeTaskDrag({ id: task.id, status: task.status }),
        );
        e.dataTransfer.effectAllowed = "move";
        setDragging(true);
      }}
      onDragOver={(e) => {
        if (!isTaskDrag(e.dataTransfer.types)) return;
        // Handle the drop here (position-aware); don't let the column's
        // append-to-end handler also fire.
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = "move";
        onReorderHover({ status: task.status, beforeId: slotBeforeId(e) });
      }}
      onDrop={(e) => {
        if (!isTaskDrag(e.dataTransfer.types)) return;
        e.preventDefault();
        e.stopPropagation();
        const payload = decodeTaskDrag(e.dataTransfer.getData(TASK_DRAG_TYPE));
        if (payload) onReorder(payload.id, task.status, slotBeforeId(e));
      }}
      onDragEnd={() => {
        setDragging(false);
        onDragEnd();
      }}
      className={cn(
        "group rounded-md border border-l-2 bg-background p-2.5 text-sm shadow-sm",
        "cursor-grab active:cursor-grabbing",
        due === "overdue" &&
          "border-l-red-500 bg-red-500/[0.03] dark:bg-red-500/[0.07]",
        due === "today" &&
          "border-l-amber-500 bg-amber-500/[0.03] dark:bg-amber-500/[0.07]",
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
            <DropdownMenuLabel className="pb-0.5 pt-1 text-muted-foreground">
              Notes
            </DropdownMenuLabel>
            <div
              className="px-2 py-1"
              // Keep the menu open and stop its typeahead from eating keystrokes.
              onKeyDown={(e) => e.stopPropagation()}
            >
              <NotesField task={task} onSetNotes={onSetNotes} />
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

      {(task.repo || task.dueTs !== undefined || hasNotes) && (
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
                due === "overdue" &&
                  "border-transparent bg-red-500/15 text-red-600 dark:text-red-400",
                due === "today" &&
                  "border-transparent bg-amber-500/15 text-amber-600 dark:text-amber-400",
              )}
            >
              <CalendarPlus aria-hidden className="size-3" />
              {fmtDay(task.dueTs)}
            </Badge>
          )}
          {hasNotes && (
            <Badge
              variant="outline"
              className="gap-1 text-[10px] text-muted-foreground"
              title={task.notes}
            >
              <StickyNote aria-hidden className="size-3" />
              Notes
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
