import { useMemo, useState } from "react";
import { Archive, MapPin, PencilLine, Plus } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import {
  fmtAge,
  fmtClock,
  storeAddTask,
  storeArchiveEmail,
  storeSetTaskDone,
  useStoreSnapshot,
  type EmailItem,
  type TaskItem,
} from "@/lib/data";

const TAG_ORDER: Record<EmailItem["tag"], number> = { needs_reply: 0, invite: 1, fyi: 2 };
const TAG_LABEL: Record<EmailItem["tag"], string> = {
  needs_reply: "needs reply",
  invite: "invite",
  fyi: "fyi",
};
const TAG_CLASS: Record<EmailItem["tag"], string> = {
  needs_reply:
    "bg-amber-500/15 text-amber-700 dark:bg-amber-500/20 dark:text-amber-400",
  invite: "bg-blue-500/15 text-blue-700 dark:bg-blue-500/20 dark:text-blue-400",
  fyi: "bg-muted text-muted-foreground",
};

function ColumnHeader({ label, note }: { label: string; note?: string }) {
  return (
    <div className="flex shrink-0 items-baseline justify-between gap-2 border-b px-3 py-2">
      <span className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
        {label}
      </span>
      {note && <span className="truncate text-xs text-muted-foreground/70">{note}</span>}
    </div>
  );
}

export function EmailCalendarScreen() {
  const { snapshot } = useStoreSnapshot();
  const now = Date.now();

  // Local optimistic overlays on the read-only snapshot.
  const [doneOverrides, setDoneOverrides] = useState<Record<number, boolean>>({});
  const [archived, setArchived] = useState<Set<number>>(new Set());
  const [addedTasks, setAddedTasks] = useState<TaskItem[]>([]);
  const [draft, setDraft] = useState("");

  const todayEvents = useMemo(() => {
    const today = new Date(now).toDateString();
    return snapshot.events
      .filter((e) => new Date(e.startTs).toDateString() === today)
      .sort((a, b) => a.startTs - b.startTs);
  }, [snapshot.events, now]);

  const nextEventTs = useMemo(() => {
    const upcoming = snapshot.events
      .filter((e) => e.startTs > now)
      .sort((a, b) => a.startTs - b.startTs);
    return upcoming[0]?.startTs;
  }, [snapshot.events, now]);

  const emails = useMemo(
    () =>
      snapshot.emails
        .filter((e) => !e.archived && !archived.has(e.id))
        .sort((a, b) => TAG_ORDER[a.tag] - TAG_ORDER[b.tag] || b.receivedTs - a.receivedTs),
    [snapshot.emails, archived],
  );

  const emailRun = snapshot.runs.find((r) => r.collector === "claude:email");

  const tasks = useMemo(() => {
    const merged = [...addedTasks, ...snapshot.tasks];
    const withDone = merged.map((t) => ({ ...t, done: doneOverrides[t.id] ?? t.done }));
    const open = withDone.filter((t) => !t.done).sort((a, b) => a.createdAt - b.createdAt);
    const done = withDone.filter((t) => t.done);
    return { open, done };
  }, [snapshot.tasks, addedTasks, doneOverrides]);

  function toggleTask(id: number, done: boolean) {
    setDoneOverrides((prev) => ({ ...prev, [id]: done }));
    void storeSetTaskDone(id, done);
  }

  function archiveEmail(id: number) {
    setArchived((prev) => new Set(prev).add(id));
    void storeArchiveEmail(id);
  }

  function addTask() {
    const text = draft.trim();
    if (!text) return;
    setAddedTasks((prev) => [
      { id: -Date.now(), source: "manual", text, done: false, createdAt: Date.now() },
      ...prev,
    ]);
    setDraft("");
    void storeAddTask(text);
  }

  function draftReply(email: EmailItem) {
    const prompt = `claude -p "draft a reply to ${email.fromName} re: ${email.subject}"`;
    void navigator.clipboard?.writeText(prompt);
    toast.success("prompt copied");
  }

  return (
    <div className="grid h-full min-h-0 grid-cols-1 lg:grid-cols-[3fr_5fr_3fr]">
      {/* Schedule */}
      <div className="flex min-h-0 flex-col border-b lg:border-b-0 lg:border-r">
        <ColumnHeader label="Schedule" note={new Date(now).toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" })} />
        <ScrollArea className="flex-1">
          <div className="flex flex-col p-2">
            {todayEvents.length === 0 && (
              <p className="px-2 py-6 text-center text-sm text-muted-foreground">
                Nothing on the calendar today.
              </p>
            )}
            {todayEvents.map((event, i) => {
              const past = (event.endTs ?? event.startTs) < now;
              const isNext = event.startTs === nextEventTs;
              // Drop a "now" line just before the first event that hasn't started.
              const showNowLine =
                event.startTs > now && (i === 0 || todayEvents[i - 1].startTs <= now);
              return (
                <div key={event.id}>
                  {showNowLine && (
                    <div className="flex items-center gap-2 px-2 py-1">
                      <span className="size-1.5 rounded-full bg-red-500" />
                      <span className="text-xs font-medium text-red-500">{fmtClock(now)}</span>
                      <span className="h-px flex-1 bg-red-500/40" />
                    </div>
                  )}
                  <div
                    className={cn(
                      "flex flex-col gap-0.5 rounded-md px-2 py-1.5",
                      past && "opacity-50",
                      isNext && "bg-accent",
                    )}
                  >
                    <div className="flex items-baseline gap-2">
                      <span className="w-16 shrink-0 font-mono text-xs text-muted-foreground">
                        {fmtClock(event.startTs)}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-sm font-medium">
                        {event.title}
                      </span>
                    </div>
                    <div className="flex items-center gap-2 pl-16 text-xs text-muted-foreground">
                      <span className="truncate">{event.attendees.join(", ")}</span>
                      {event.location && (
                        <span className="flex shrink-0 items-center gap-0.5">
                          <MapPin className="size-3" />
                          {event.location}
                        </span>
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </ScrollArea>
      </div>

      {/* Inbox */}
      <div className="flex min-h-0 flex-col border-b lg:border-b-0 lg:border-r">
        <ColumnHeader
          label="Inbox"
          note={emailRun ? `via claude -p · ${fmtAge(emailRun.ranAt, now)}` : undefined}
        />
        <ScrollArea className="flex-1">
          <div className="flex flex-col divide-y">
            {emails.length === 0 && (
              <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                Inbox zero. Nice.
              </p>
            )}
            {emails.map((email) => (
              <div
                key={email.id}
                className={cn(
                  "flex flex-col gap-1 px-3 py-2.5",
                  email.tag === "fyi" && "opacity-70",
                )}
              >
                <div className="flex items-center gap-2">
                  <Badge className={cn("shrink-0", TAG_CLASS[email.tag])}>
                    {TAG_LABEL[email.tag]}
                  </Badge>
                  <span className="min-w-0 flex-1 truncate text-sm font-semibold">
                    {email.fromName}
                  </span>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {fmtAge(email.receivedTs, now)}
                  </span>
                </div>
                <div className="truncate text-sm">{email.subject}</div>
                <div className="truncate text-xs text-muted-foreground">✦ {email.summary}</div>
                <div className="mt-0.5 flex items-center gap-1.5">
                  {email.tag === "needs_reply" && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="h-6 px-2 text-xs"
                      onClick={() => draftReply(email)}
                    >
                      <PencilLine className="size-3" /> Draft reply
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-6 px-2 text-xs text-muted-foreground"
                    onClick={() => archiveEmail(email.id)}
                  >
                    <Archive className="size-3" /> Archive
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* Tasks */}
      <div className="flex min-h-0 flex-col">
        <ColumnHeader label="Tasks" />
        <div className="flex shrink-0 items-center gap-1.5 border-b px-3 py-2">
          <Input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addTask();
            }}
            placeholder="Add a task…"
            className="h-7 text-sm"
          />
          <Button variant="ghost" size="icon-sm" aria-label="Add task" onClick={addTask}>
            <Plus />
          </Button>
        </div>
        <ScrollArea className="flex-1">
          <div className="flex flex-col p-2">
            {tasks.open.map((task) => (
              <TaskRow key={task.id} task={task} now={now} nextEventTs={nextEventTs} onToggle={toggleTask} />
            ))}
            {tasks.done.length > 0 && (
              <div className="mt-2 flex flex-col border-t pt-2">
                {tasks.done.map((task) => (
                  <TaskRow
                    key={task.id}
                    task={task}
                    now={now}
                    nextEventTs={nextEventTs}
                    onToggle={toggleTask}
                  />
                ))}
              </div>
            )}
          </div>
        </ScrollArea>
      </div>
    </div>
  );
}

function TaskRow({
  task,
  now,
  nextEventTs,
  onToggle,
}: {
  task: TaskItem;
  now: number;
  nextEventTs?: number;
  onToggle: (id: number, done: boolean) => void;
}) {
  const urgent =
    task.dueTs !== undefined &&
    !task.done &&
    (task.dueTs < now || (nextEventTs !== undefined && task.dueTs <= nextEventTs));
  return (
    <label className="flex cursor-pointer items-start gap-2 rounded-md px-2 py-1.5 hover:bg-accent/50">
      <input
        type="checkbox"
        checked={task.done}
        onChange={(e) => onToggle(task.id, e.target.checked)}
        className="mt-0.5 size-3.5 shrink-0 accent-primary"
      />
      <span className="min-w-0 flex-1">
        <span className={cn("text-sm", task.done && "text-muted-foreground line-through")}>
          {task.text}
        </span>
        {task.dueTs !== undefined && !task.done && (
          <Badge
            variant="outline"
            className={cn(
              "ml-2 align-middle",
              urgent && "border-transparent bg-red-500/15 text-red-600 dark:text-red-400",
            )}
          >
            {fmtClock(task.dueTs)}
          </Badge>
        )}
      </span>
    </label>
  );
}
