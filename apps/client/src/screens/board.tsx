import { Fragment, useCallback, useMemo, useRef, useState } from "react";
import {
  CalendarPlus,
  FolderGit2,
  GripVertical,
  ListTodo,
  MoreHorizontal,
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
import { Switch } from "@/components/ui/switch";
import { ownerRepoFromOrigin, useAgentboardState } from "@/lib/agentboard";
import { repoAccentStyles, repoIcon, type RepoMeta } from "@/lib/repo-identity";
import { useBoardGroupByRepo } from "@/lib/board-prefs";
import { uiAction } from "@/lib/ui-action";
import { cn } from "@/lib/utils";
import {
  fmtDay,
  storeAttachTaskIssue,
  storeAttachTaskPr,
  storeClearDone,
  storeDeleteTask,
  storeDetachTaskIssue,
  storeDetachTaskPr,
  storePromoteTaskToIssue,
  storeSetTaskPosition,
  storeSetTaskStatus,
  storeUpdateTask,
  TASK_STATUS_LABEL,
  TASK_STATUSES,
  useStoreSnapshot,
  type IssueItem,
  type PrItem,
  type TaskIssueLink,
  type TaskItem,
  type TaskPrLink,
  type TaskStatus,
} from "@/lib/data";
import {
  decodeTaskDrag,
  encodeTaskDrag,
  isTaskDrag,
  reorderedPosition,
  TASK_DRAG_TYPE,
} from "@/lib/kanban-dnd";
import { countByStatus, dueState, overdueByStatus } from "@/lib/board-metrics";
import { matchesTaskFilter } from "@/lib/board-filter";
import {
  bucketByStatus,
  byBoardOrder,
  groupTasksByRepo,
  NO_REPO_GROUP,
  railRepoKeyForTask,
  repoGroupLabel,
  taskRepoKey,
} from "@/lib/board-groups";
import { useFocusTarget } from "@/lib/focus-target";
import { useNow } from "@/lib/now";
import { openExternalUrl } from "@/lib/open-url";
import { PR_TONE, prTone } from "@/lib/pr-tone";
import { useShortcuts } from "@/lib/shortcuts";
import { toast } from "sonner";
import type { Result } from "better-result";
import type { IpcError } from "@/lib/errors";
import { useWorkspace } from "@/lib/workspace";

/**
 * Fire a board mutation and report a failure. Every card action here paints an
 * optimistic overlay first, so a dropped write would otherwise look like it
 * worked right up until the next snapshot quietly reverted it.
 */
async function commit(mutation: Promise<Result<unknown, IpcError>>, what: string): Promise<void> {
  const done = await mutation;
  if (done.isErr()) toast.error(`Couldn't ${what} — ${done.error.message}`);
}

/** Optimistic edits (text/notes/due) applied over a snapshot todo until it
 * re-arrives. */
type TaskEdit = { text?: string; notes?: string | undefined; dueTs?: number | undefined };

/** Optimistic status + fractional position from a drag-reorder, until re-arrival. */
type PosOverride = { status: TaskStatus; position: number };

/** The slot a card would drop into: before `beforeId`, or at a column's end. */
type DropSlot = { status: TaskStatus; beforeId: number | "end" };

/** The synthetic lane key when swimlane grouping is toggled off — one unnamed
 * lane holding every card. Never a real repo key (`taskRepoKey` returns
 * `owner/name`, a path basename, or `NO_REPO_GROUP`). */
const ALL_TASKS_LANE = "__all_tasks__";

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
 * Board — the cross-repo kanban for finding and watching work in flight.
 *
 * Columns are the five task statuses; rows are automatic per-repo swimlanes
 * derived from each task's repo binding (see `lib/board-groups.ts`), so a lane
 * appears and disappears with its work rather than being managed by hand. The
 * header's Swimlanes switch (persisted as `agentboard.boardGroupByRepo`)
 * flattens the board to one unnamed lane; cards carry their repo's identity
 * (icon/color/tint) either way.
 *
 * **This screen does not create tasks** — the Agentboard's `+` flow is the only
 * creator, so a task and the repo it belongs to are established together at
 * submit. Here a card can be moved, reordered, renamed, given a due date,
 * linked to issues/PRs, promoted to a GitHub issue, or deleted. Read-only over
 * the snapshot with local optimistic overlays for moves, edits and deletes
 * until the next `store://snapshot` arrives.
 */
export function BoardScreen() {
  const { snapshot } = useStoreSnapshot();
  const { activeTab, openTabWithFocus } = useWorkspace();
  const now = useNow();
  // Deep-link focus: a promoted-todo / board deep link scrolls the card here.
  const focusRef = useFocusTarget<HTMLDivElement>("board");
  const filterInputRef = useRef<HTMLInputElement>(null);

  const [statusOverrides, setStatusOverrides] = useState<Record<number, TaskStatus>>({});
  const [posOverrides, setPosOverrides] = useState<Record<number, PosOverride>>({});
  const [editOverrides, setEditOverrides] = useState<Record<number, TaskEdit>>({});
  const [deletedIds, setDeletedIds] = useState<Set<number>>(() => new Set());
  // Quick filter: case-insensitive substring over each todo's text + repo tag.
  const [filter, setFilter] = useState("");

  // Board-scoped shortcuts (see lib/shortcuts.tsx for the registry). Gated on
  // the tab being active: this screen stays mounted while hidden, so without
  // the gate "n"/"/" would steal keystrokes from whatever tab is showing.
  useShortcuts(
    useMemo(
      () => ({
        "board-filter": () => filterInputRef.current?.focus(),
      }),
      [],
    ),
    activeTab === "board",
  );
  // The insertion slot the current drag would land in: drives both the column
  // highlight (`dropSlot.status`) and the drop line before `beforeId`.
  const [dropSlot, setDropSlot] = useState<DropSlot | null>(null);
  // Stable identity so every card shares one `onDragEnd` instead of a fresh
  // closure per card per render.
  const clearDropSlot = useCallback(() => setDropSlot(null), []);
  const agentState = useAgentboardState();

  // Repo identity (chosen icon + color) for the swimlane headers, keyed the
  // way lanes are: GitHub `owner/name`, bridged from each tracked repo's
  // origin URL. Sourced from the same agentboard snapshot the rail renders —
  // no second poll. Only repos that actually carry a `meta` land here, so an
  // unthemed lane keeps today's plain folder glyph.
  const repoMetaByKey = useMemo(() => {
    const m = new Map<string, RepoMeta>();
    for (const r of agentState.repos) {
      if (!r.meta) continue;
      const key = ownerRepoFromOrigin(r.originUrl);
      if (key) m.set(key, r.meta);
    }
    return m;
  }, [agentState.repos]);

  // Repos we know about (from collected PRs/issues + already-linked tasks) — the
  // promote-to-issue targets.
  const repos = useMemo(() => {
    const set = new Set<string>();
    for (const p of snapshot.prs) set.add(p.repo);
    for (const i of snapshot.issues) set.add(i.repo);
    for (const t of snapshot.tasks) {
      for (const l of t.issues) set.add(l.repo);
      for (const l of t.prs) set.add(l.repo);
    }
    return [...set].toSorted();
  }, [snapshot.prs, snapshot.issues, snapshot.tasks]);

  const merged = useMemo(
    () =>
      snapshot.tasks
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
    [snapshot.tasks, editOverrides, statusOverrides, posOverrides, deletedIds],
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

  // Repo swimlanes. Grouping is automatic — a lane is just "the tasks that
  // resolved to this repo" — so lanes appear and vanish with the work and
  // there is nothing to create, name, or clean up. The only bucketing pass:
  // header totals come from `counts`, and `reorder` sorts the one column it
  // needs at drop time, so nothing here re-buckets the whole board.
  const grouped = useMemo(() => groupTasksByRepo(visible), [visible]);

  // Swimlanes are a preference: toggled off, the board is one unnamed lane
  // holding every card (each card keeps its repo glyph, so identity survives
  // the flattening). Persisted in the shared settings file.
  const [groupByRepo, setGroupByRepo] = useBoardGroupByRepo();
  const lanes = useMemo(() => {
    const groups = groupByRepo ? grouped : [{ key: ALL_TASKS_LANE, label: "", tasks: visible }];
    return groups.map((g) => ({ ...g, columns: bucketByStatus(g.tasks) }));
  }, [grouped, groupByRepo, visible]);

  // Real repos only — the "No repo" lane is a bucket, not a repo, and must
  // not inflate the header's repo count. Counted from the real grouping so
  // the swimlane toggle never changes what the header claims.
  const repoLaneCount = useMemo(
    () => grouped.filter((l) => l.key !== NO_REPO_GROUP).length,
    [grouped],
  );

  // Per-status totals for the sticky header (and the Clear-done gate).
  const counts = useMemo(() => countByStatus(visible), [visible]);

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
    void commit(storeSetTaskStatus(id, status), "move that todo");
  }

  // Reorder `id` into `status` just before `beforeId` ("end" = append). Computes
  // a fractional optimistic position from the neighbors so the card sorts into
  // its new slot immediately, and sends the integer slot index to the backend
  // (which renumbers the column). Positions are global per status in the store,
  // so the column is assembled across every lane — built here, on drop, rather
  // than kept as a second render-time bucketing of the whole board.
  function reorder(id: number, status: TaskStatus, beforeId: number | "end") {
    if (beforeId === id) return;
    const col = visible.filter((t) => t.status === status && t.id !== id).toSorted(byBoardOrder);
    const insertAt =
      beforeId === "end"
        ? col.length
        : Math.max(
            0,
            col.findIndex((t) => t.id === beforeId),
          );
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
    void commit(storeSetTaskPosition(id, status, insertAt), "reorder that todo");
  }

  function promote(id: number, repo: string) {
    void commit(storePromoteTaskToIssue(id, repo), "promote that task to an issue");
  }

  function attachIssue(id: number, issue: IssueItem) {
    void commit(storeAttachTaskIssue(id, issue.repo, issue.number, issue.url), "attach that issue");
  }

  function detachIssue(id: number, link: TaskIssueLink) {
    void commit(storeDetachTaskIssue(id, link.repo, link.number), "detach that issue");
  }

  function attachPr(id: number, pr: PrItem) {
    void commit(storeAttachTaskPr(id, pr.repo, pr.number, pr.url), "attach that PR");
  }

  function detachPr(id: number, link: TaskPrLink) {
    void commit(storeDetachTaskPr(id, link.repo, link.number), "detach that PR");
  }

  // Rename and due-date edits both re-send the todo's other free-form fields
  // (`storeUpdateTask` is a full replace of text/notes/due), reading them from
  // `merged` so chained optimistic edits compose.
  function rename(id: number, text: string) {
    const trimmed = text.trim();
    const current = merged.find((t) => t.id === id);
    if (!current || !trimmed || trimmed === current.text) return;
    setEditOverrides((prev) => ({ ...prev, [id]: { ...prev[id], text: trimmed } }));
    void commit(storeUpdateTask(id, trimmed, current.notes, current.dueTs), "rename that todo");
  }

  function setDue(id: number, dueTs: number | undefined) {
    const current = merged.find((t) => t.id === id);
    if (!current) return;
    setEditOverrides((prev) => ({ ...prev, [id]: { ...prev[id], dueTs } }));
    void commit(storeUpdateTask(id, current.text, current.notes, dueTs), "set that due date");
  }

  function setNotes(id: number, notes: string) {
    const current = merged.find((t) => t.id === id);
    if (!current) return;
    // Empty/whitespace-only notes clear the field back to unset.
    const value = notes.trim() === "" ? undefined : notes;
    setEditOverrides((prev) => ({ ...prev, [id]: { ...prev[id], notes: value } }));
    void commit(storeUpdateTask(id, current.text, value, current.dueTs), "save those notes");
  }

  function remove(id: number) {
    setDeletedIds((prev) => new Set(prev).add(id));
    void commit(storeDeleteTask(id), "delete that todo");
  }

  function clearDone() {
    void commit(storeClearDone(), "clear the Done column");
  }

  // Jump to a card's repo on the Agentboard: same focus primitive the
  // needs-you popover uses — land on the screen, scroll the rail row into
  // view, flash it. `railKey` is resolved per card at render, so a card whose
  // repo isn't on the rail simply doesn't offer the jump.
  const openOnAgentboard = useCallback(
    (railKey: string) => {
      uiAction("board.open_agentboard", "board");
      openTabWithFocus({ screen: "agentboard", kind: "repo", id: railKey });
    },
    [openTabWithFocus],
  );

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
        <h2 className="text-sm font-medium">Board</h2>
        <span className="text-xs text-muted-foreground">
          {repoLaneCount} {repoLaneCount === 1 ? "repo" : "repos"} · {snapshot.tasks.length} tasks
        </span>
        {filter.trim() !== "" && hiddenCount > 0 && (
          <span className="text-xs text-muted-foreground">{hiddenCount} hidden</span>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          {/* `htmlFor`, never a wrapping label: the switch is a <button>, and a
              label wrapped around it forwards each click back into it — two
              toggles per click, net nothing. */}
          <label htmlFor="board-swimlanes" className="cursor-pointer text-xs text-muted-foreground">
            Swimlanes
          </label>
          <Switch
            id="board-swimlanes"
            checked={groupByRepo}
            onCheckedChange={(v) => {
              setGroupByRepo(v);
              uiAction("board.group_by_repo", "board", v ? "on" : "off");
            }}
            aria-label="Group tasks into repo swimlanes"
          />
          {counts.done > 0 && (
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground"
              title="Remove done tasks completed over 7 days ago"
              onClick={clearDone}
            >
              Clear done
            </Button>
          )}
          <div className="relative w-44">
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              ref={filterInputRef}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") setFilter("");
              }}
              placeholder="Filter…"
              className="h-7 pl-7 text-sm"
              spellCheck={false}
              aria-label="Filter tasks"
            />
          </div>
        </div>
      </div>

      {isEmpty ? (
        <div ref={focusRef} className="flex min-h-0 flex-1 items-center justify-center p-6">
          <div className="flex max-w-sm flex-col items-center gap-2 text-center">
            <ListTodo aria-hidden className="size-8 text-muted-foreground/50" />
            <p className="text-sm font-medium">No tasks yet</p>
            <p className="text-xs text-muted-foreground">
              Tasks are created on the{" "}
              <span className="font-medium text-foreground">Agentboard</span> — hit{" "}
              <span className="font-medium text-foreground">+</span> on a repo to start one. It
              shows up here, in that repo&apos;s lane, the moment you submit.
            </p>
          </div>
        </div>
      ) : (
        <ScrollArea className="min-h-0 flex-1">
          <div ref={focusRef} className="min-w-[900px] p-3">
            {/* One status header for the whole board — the columns are shared
                across every lane, so repeating the labels per lane would be
                four-fifths noise. Sticky so they stay readable while scrolling
                a long list of repos. */}
            <div className="sticky top-0 z-10 grid grid-cols-5 gap-3 bg-background pb-2">
              {TASK_STATUSES.map((status) => (
                <div key={status} className="flex items-center justify-between gap-1 px-2.5">
                  <span className="truncate text-xs font-medium uppercase tracking-wide text-muted-foreground">
                    {TASK_STATUS_LABEL[status]}
                  </span>
                  <span className="flex items-center gap-1">
                    {overdue[status] > 0 && (
                      <span
                        title={`${overdue[status]} overdue`}
                        className="rounded-full bg-red-500/15 px-1.5 font-mono text-[10px] text-red-600 dark:text-red-400"
                      >
                        {overdue[status]} late
                      </span>
                    )}
                    <span className="rounded-full bg-muted px-1.5 font-mono text-[10px] text-muted-foreground">
                      {counts[status]}
                    </span>
                  </span>
                </div>
              ))}
            </div>

            <div className="flex flex-col gap-4">
              {lanes.map((lane) => {
                const laneMeta =
                  lane.key === ALL_TASKS_LANE ? undefined : repoMetaByKey.get(lane.key);
                const laneAccent = repoAccentStyles(laneMeta);
                return (
                  <section key={lane.key}>
                    {/* Flat mode's single lane is every repo's, so it gets no
                      header — the sticky status row above is enough. */}
                    {lane.key !== ALL_TASKS_LANE && (
                      <div
                        // The lane header is the repo-identity surface on this
                        // screen: colored edge always (when themed), plus the
                        // soft wash for `style: "tint"`. Transparent edge keeps
                        // unthemed lanes aligned with themed ones.
                        className="mb-1 flex items-center gap-1.5 rounded-md border-l-2 border-l-transparent px-1.5 py-1"
                        style={{ ...laneAccent.edgeStyle, ...laneAccent.surfaceStyle }}
                      >
                        <LaneGlyph meta={laneMeta} />
                        <span
                          className={cn(
                            // `pr-px`: the italic variant's final glyph overhangs
                            // its content box, and `truncate`'s overflow:hidden
                            // clips the overhang without ever showing an ellipsis.
                            "truncate pr-px text-sm font-semibold",
                            lane.key === NO_REPO_GROUP &&
                              "font-normal italic text-muted-foreground",
                          )}
                          title={lane.key === NO_REPO_GROUP ? undefined : lane.key}
                        >
                          {lane.label}
                        </span>
                        <span className="rounded-full bg-muted px-1.5 font-mono text-[10px] text-muted-foreground">
                          {lane.tasks.length}
                        </span>
                      </div>
                    )}
                    <div className="grid grid-cols-5 gap-3">
                      {TASK_STATUSES.map((status) => (
                        <div
                          key={status}
                          onDragOver={(e) => {
                            if (!isTaskDrag(e.dataTransfer.types)) return;
                            e.preventDefault();
                            e.dataTransfer.dropEffect = "move";
                            // Over a cell's empty tail (cards handle their own
                            // hover and stop propagation) — append to the column.
                            // Keep identity when unchanged: dragover fires
                            // continuously, and a fresh object every event would
                            // re-render all lanes for the whole drag.
                            setDropSlot((cur) =>
                              cur?.status === status && cur.beforeId === "end"
                                ? cur
                                : { status, beforeId: "end" },
                            );
                          }}
                          onDragLeave={(e) => {
                            // Ignore moves between children of the same cell.
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
                            "flex min-h-12 flex-col gap-2 rounded-lg border bg-muted/30 p-2",
                            // Highlights the status across every lane, because
                            // that is what a drop actually changes: a card's repo
                            // comes from its slot/links, so dropping into another
                            // repo's lane can't move it there.
                            dropSlot?.status === status &&
                              "border-violet-500/60 bg-violet-500/5 dark:bg-violet-500/10",
                          )}
                        >
                          {lane.columns[status].map((task, i) => {
                            const repoKey = taskRepoKey(task);
                            const railKey = railRepoKeyForTask(agentState.repos, task);
                            return (
                              <Fragment key={task.id}>
                                <DropLine
                                  active={
                                    dropSlot?.status === status && dropSlot.beforeId === task.id
                                  }
                                />
                                <Card
                                  task={task}
                                  now={now}
                                  repos={repos}
                                  repoMeta={repoMetaByKey.get(repoKey)}
                                  repoLabel={
                                    groupByRepo || repoKey === NO_REPO_GROUP
                                      ? undefined
                                      : repoGroupLabel(repoKey)
                                  }
                                  onOpenAgentboard={
                                    railKey ? () => openOnAgentboard(railKey) : undefined
                                  }
                                  openIssues={snapshot.issues}
                                  openPrs={snapshot.prs}
                                  nextId={lane.columns[status][i + 1]?.id ?? null}
                                  onMove={move}
                                  onReorderHover={setDropSlot}
                                  onReorder={reorder}
                                  onPromote={promote}
                                  onAttachIssue={attachIssue}
                                  onDetachIssue={detachIssue}
                                  onAttachPr={attachPr}
                                  onDetachPr={detachPr}
                                  onRename={rename}
                                  onSetDue={setDue}
                                  onSetNotes={setNotes}
                                  onDelete={remove}
                                  onDragEnd={clearDropSlot}
                                />
                              </Fragment>
                            );
                          })}
                          <DropLine
                            active={dropSlot?.status === status && dropSlot.beforeId === "end"}
                          />
                        </div>
                      ))}
                    </div>
                  </section>
                );
              })}
            </div>
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

/** A swimlane's repo glyph: the repo's chosen icon tinted with its color, or
 * the plain folder glyph when it has no identity set. */
function LaneGlyph({ meta }: { meta?: RepoMeta }) {
  if (!meta) {
    return <FolderGit2 aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />;
  }
  const Icon = repoIcon(meta);
  return (
    <Icon
      aria-hidden
      className="size-3.5 shrink-0 text-muted-foreground"
      style={repoAccentStyles(meta).iconStyle}
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
  repoMeta,
  repoLabel,
  onOpenAgentboard,
  openIssues,
  openPrs,
  nextId,
  onMove,
  onReorderHover,
  onReorder,
  onPromote,
  onAttachIssue,
  onDetachIssue,
  onAttachPr,
  onDetachPr,
  onRename,
  onSetDue,
  onSetNotes,
  onDelete,
  onDragEnd,
}: {
  task: TaskItem;
  now: number;
  repos: string[];
  /** The chosen icon/color of the repo this task resolved to, when that repo
   * has one. Undefined (no repo, or an unthemed repo) renders the card
   * exactly as it did before repo identity existed. */
  repoMeta?: RepoMeta;
  /** The repo name to print on the card — set only in flat (no-swimlane) mode,
   * where no lane header identifies the repo. Undefined in grouped mode and
   * for no-repo tasks. */
  repoLabel?: string;
  /** Jump to this task's repo row on the Agentboard rail. Undefined when the
   * repo isn't on the rail — the affordances don't render. */
  onOpenAgentboard?: () => void;
  /** Collected open issues — the "Attach issue…" candidates. */
  openIssues: IssueItem[];
  /** Collected PRs — the "Attach PR…" candidates. */
  openPrs: PrItem[];
  /** The card below this one in the column (for a bottom-half drop), or null. */
  nextId: number | null;
  onMove: (id: number, status: TaskStatus) => void;
  onReorderHover: (slot: DropSlot) => void;
  onReorder: (id: number, status: TaskStatus, beforeId: number | "end") => void;
  onPromote: (id: number, repo: string) => void;
  onAttachIssue: (id: number, issue: IssueItem) => void;
  onDetachIssue: (id: number, link: TaskIssueLink) => void;
  onAttachPr: (id: number, pr: PrItem) => void;
  onDetachPr: (id: number, link: TaskPrLink) => void;
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
  // Attach candidates: collected refs not already linked to this task.
  const attachableIssues = openIssues.filter(
    (i) => !task.issues.some((l) => l.repo === i.repo && l.number === i.number),
  );
  const attachablePrs = openPrs.filter(
    (p) => !task.prs.some((l) => l.repo === p.repo && l.number === p.number),
  );
  const hasLinks = task.issues.length > 0 || task.prs.length > 0;
  // Repo identity on the card: tinted glyph, colored edge, and (for
  // `style: "tint"`) a background wash mixed into the card's own opaque
  // background. The due-state accents own the edge and wash when present —
  // identity never outranks attention — so both apply only when due is quiet.
  const accent = repoAccentStyles(repoMeta, "var(--background)");
  const RepoGlyph = repoMeta ? repoIcon(repoMeta) : null;
  const identityStyle =
    due === "none" ? { ...accent.edgeStyle, ...accent.surfaceStyle } : undefined;
  // The identity row's text: `repo · ⎇ branch`, either part optional.
  const branch = task.slot?.branch;
  const detached = branch !== undefined && !task.slot?.dir;
  const identityRowText = [repoLabel, branch && `⎇ ${branch}${detached ? " · detached" : ""}`]
    .filter(Boolean)
    .join(" · ");

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
        due === "overdue" && "border-l-red-500 bg-red-500/[0.03] dark:bg-red-500/[0.07]",
        due === "today" && "border-l-amber-500 bg-amber-500/[0.03] dark:bg-amber-500/[0.07]",
        task.status === "done" && "opacity-60",
        dragging && "opacity-40",
      )}
      style={identityStyle}
    >
      <div className="flex items-start gap-1.5">
        <GripVertical
          aria-hidden
          className="-ml-1 mt-0.5 size-3.5 shrink-0 text-muted-foreground/40 opacity-0 transition-opacity group-hover:opacity-100"
        />
        {RepoGlyph && (
          <RepoGlyph
            aria-hidden
            className="mt-0.5 size-3.5 shrink-0 text-muted-foreground"
            style={accent.iconStyle}
          />
        )}
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
            {onOpenAgentboard && (
              <>
                <DropdownMenuItem onSelect={onOpenAgentboard}>Open on Agentboard</DropdownMenuItem>
                <DropdownMenuSeparator />
              </>
            )}
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
            {attachableIssues.length > 0 && (
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Attach issue…</DropdownMenuSubTrigger>
                <DropdownMenuSubContent className="max-h-72 overflow-y-auto">
                  {attachableIssues.map((issue) => (
                    <DropdownMenuItem
                      key={`${issue.repo}#${issue.number}`}
                      onSelect={() => onAttachIssue(task.id, issue)}
                    >
                      <span className="mr-1.5 font-mono text-muted-foreground">
                        #{issue.number}
                      </span>
                      <span className="max-w-56 truncate">{issue.title}</span>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            )}
            {attachablePrs.length > 0 && (
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Attach PR…</DropdownMenuSubTrigger>
                <DropdownMenuSubContent className="max-h-72 overflow-y-auto">
                  {attachablePrs.map((pr) => (
                    <DropdownMenuItem
                      key={`${pr.repo}#${pr.number}`}
                      onSelect={() => onAttachPr(task.id, pr)}
                    >
                      <span className="mr-1.5 font-mono text-muted-foreground">#{pr.number}</span>
                      <span className="max-w-56 truncate">{pr.title}</span>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            )}
            {hasLinks && (
              <DropdownMenuSub>
                <DropdownMenuSubTrigger>Detach…</DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  {task.issues.map((link) => (
                    <DropdownMenuItem
                      key={`i${link.repo}#${link.number}`}
                      onSelect={() => onDetachIssue(task.id, link)}
                    >
                      issue #{link.number} · {link.repo}
                    </DropdownMenuItem>
                  ))}
                  {task.prs.map((link) => (
                    <DropdownMenuItem
                      key={`p${link.repo}#${link.number}`}
                      onSelect={() => onDetachPr(task.id, link)}
                    >
                      PR #{link.number} · {link.repo}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            )}
            {repos.length > 0 ? (
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
            <DropdownMenuItem variant="destructive" onSelect={() => setConfirmingDelete(true)}>
              Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {/* The identity row: the repo name in flat mode (where no lane header
          says it), plus the slot branch when one exists — a branchless task in
          grouped mode renders nothing here, its lane header already identifies
          it. Clickable when the repo has an Agentboard rail row. */}
      {(repoLabel !== undefined || branch) && (
        <div
          className={cn(
            "mt-1.5 flex items-center font-mono text-[11px] text-muted-foreground",
            detached && "italic text-muted-foreground/70",
          )}
        >
          {onOpenAgentboard ? (
            <button
              type="button"
              onClick={onOpenAgentboard}
              title={`Open on Agentboard${task.slot?.dir ? ` — ${task.slot.dir}` : ""}`}
              className="min-w-0 truncate text-left hover:text-foreground hover:underline"
            >
              {identityRowText}
            </button>
          ) : (
            <span
              className="min-w-0 truncate"
              title={task.slot?.dir ?? (branch ? `worktree removed — branch ${branch}` : undefined)}
            >
              {identityRowText}
            </span>
          )}
        </div>
      )}
      {(hasLinks || task.dueTs !== undefined || hasNotes) && (
        <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
          {task.issues.map((link) => (
            <a
              key={`i${link.repo}#${link.number}`}
              href={link.url}
              target="_blank"
              rel="noreferrer"
              onClick={(e) => {
                e.preventDefault();
                void openExternalUrl(link.url);
              }}
              title={`${link.repo}#${link.number} · ${link.state}`}
              className={cn(
                "font-mono text-[11px] text-muted-foreground hover:text-foreground hover:underline",
                link.state === "closed" && "line-through opacity-60",
              )}
            >
              #{link.number}
            </a>
          ))}
          {task.prs.map((link) => {
            // Compact link chips stay muted unless something is settled or
            // wrong — running/passing/plain would be noise at this size.
            const tone = prTone(link);
            return (
              <a
                key={`p${link.repo}#${link.number}`}
                href={link.url}
                target="_blank"
                rel="noreferrer"
                onClick={(e) => {
                  e.preventDefault();
                  void openExternalUrl(link.url);
                }}
                title={`${link.repo}#${link.number} · ${link.state}${link.checks !== "none" ? ` · checks ${link.checks}` : ""}`}
                className={cn(
                  "font-mono text-[11px] hover:underline",
                  tone === "merged" || tone === "failed"
                    ? PR_TONE[tone].text
                    : "text-muted-foreground hover:text-foreground",
                )}
              >
                PR #{link.number}
                {link.state === "merged" && " ✓"}
              </a>
            );
          })}
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
            <AlertDialogTitle>Delete this task?</AlertDialogTitle>
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
