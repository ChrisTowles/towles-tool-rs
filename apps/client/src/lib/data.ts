import { useEffect, useState } from "react";
import type { Result } from "better-result";
import type { IpcError } from "./errors";
import { invoke, isTauri } from "./tauri";

/**
 * Client-side view of the personal-HQ store (the Rust `tt-store` crate, surfaced
 * by the Tauri app). Mirrors the serialized snapshot (camelCase) that the
 * `store_snapshot` command returns and the `store://snapshot` event broadcasts.
 * Timestamps are epoch milliseconds, except calendar events, whose `start`/`end`
 * are RFC 3339 with the calendar's offset (see {@link CalEvent}). Before the
 * store answers the hook holds an
 * empty snapshot with `live: false`; outside Tauri (plain-Vite browser dev) it
 * holds {@link mockSnapshot} instead, still with `live: false`.
 */

/**
 * A calendar event exactly as the backend sends it: `start`/`end` are RFC 3339
 * strings carrying the offset the calendar reported
 * (`"2026-07-20T15:00:00+01:00"`). Kept separate from {@link CalEvent} because
 * nothing in the UI wants to do arithmetic on a string.
 */
export type WireCalEvent = {
  id: number;
  source: string;
  externalId: string;
  title: string;
  start: string;
  end?: string;
  attendees: string[];
  location?: string;
  joinUrl?: string;
};

/**
 * A calendar event as the screens use it: the wire shape plus epoch-ms
 * `startTs`/`endTs`, parsed once at the snapshot boundary by
 * {@link toCalEvent}.
 *
 * Both live here on purpose. Every consumer does instant arithmetic
 * (`startTs - now`, sorting, "is it live"), which wants a number; but the
 * original `start` string is the only thing that records the meeting was booked
 * as 3pm *there*, so throwing it away at the boundary would discard exactly
 * what the RFC 3339 change was made to preserve.
 */
export type CalEvent = {
  id: number;
  /**
   * Which configured calendar this came from (`"google"`, `"outlook"`). Events
   * from several calendars are merged into one timeline, so this is the only
   * way to tell a personal meeting from a work one.
   */
  source: string;
  externalId: string;
  title: string;
  /** RFC 3339 with the calendar's own offset — presentation and provenance. */
  start: string;
  end?: string;
  /** `start` as epoch ms. Derived; the instant, with the offset dropped. */
  startTs: number;
  /** `end` as epoch ms, when the event has one. */
  endTs?: number;
  attendees: string[];
  location?: string;
  joinUrl?: string;
};

/**
 * Parse one wire event into the view shape.
 *
 * An unparseable `start` yields `NaN`, which would quietly poison every
 * countdown and sort it touches, so such a row is dropped by
 * {@link toCalEvents} instead — the backend only ever writes parseable values,
 * so this means the row was hand-edited.
 */
function toCalEvent(e: WireCalEvent): CalEvent | null {
  const startTs = Date.parse(e.start);
  if (Number.isNaN(startTs)) return null;
  const endMs = e.end === undefined ? undefined : Date.parse(e.end);
  return {
    ...e,
    startTs,
    endTs: endMs !== undefined && !Number.isNaN(endMs) ? endMs : undefined,
  };
}

/** Parse a snapshot's events, dropping any row whose `start` doesn't parse. */
export function toCalEvents(events: WireCalEvent[]): CalEvent[] {
  return events.map(toCalEvent).filter((e): e is CalEvent => e !== null);
}

/** Kanban columns a todo can live in, in board order. */
export const TASK_STATUSES = ["backlog", "next", "doing", "review", "done"] as const;
export type TaskStatus = (typeof TASK_STATUSES)[number];

/** Human labels for each kanban column. */
export const TASK_STATUS_LABEL: Record<TaskStatus, string> = {
  backlog: "Backlog",
  next: "Up next",
  doing: "In progress",
  review: "In review",
  done: "Done",
};

/** One GitHub issue linked to a task; `state` is the last observed state. */
export type TaskIssueLink = {
  repo: string;
  number: number;
  url: string;
  state: "open" | "closed" | (string & {});
};

/** One GitHub PR linked to a task. */
export type TaskPrLink = {
  repo: string;
  number: number;
  url: string;
  state: "open" | "merged" | "closed" | (string & {});
  checks: string;
};

/**
 * A task's repo binding, and the task its work happens in once one exists.
 *
 * `repoRoot` is the only required part: an Agentboard task knows its repo from
 * the moment of submit, so `branch` is absent until a worktree is created (and
 * absent forever for a "task only" submit). That's what puts every task in a
 * repo swimlane on the Board.
 *
 * `repoRoot`/`branch` survive worktree removal; `dir` is cleared when the worktree
 * is removed (a detached task).
 */
export type TaskWorktree = {
  repoRoot: string;
  repo?: string;
  branch?: string;
  dir?: string;
};

/** A task — the unit of work (#339): 0..N issues, 0..N PRs, usually a task. */
export type TaskItem = {
  id: number;
  text: string;
  status: TaskStatus;
  position: number;
  createdAt: number;
  completedAt?: number;
  /** Free-form context attached to the task. */
  notes?: string;
  worktree?: TaskWorktree;
  issues: TaskIssueLink[];
  prs: TaskPrLink[];
};

export type IssueItem = {
  repo: string;
  number: number;
  title: string;
  labels: string[];
  state: string;
  url: string;
  updatedTs: number;
};

export type PrItem = {
  repo: string;
  number: number;
  title: string;
  branch: string;
  state: string;
  checks: string;
  reviewState: string;
  url: string;
  updatedTs: number;
};

export type CollectRun = {
  collector: string;
  ranAt: number;
  ok: boolean;
  message?: string;
};

/**
 * Latest state of a watched Slack DM (the `slack:dm` collector). `fromMe`
 * means the newest message is the user's own (already answered); `dismissedTs`
 * is the last message the user marked handled. A banner shows only while
 * `!fromMe && dismissedTs < ts`.
 */
export type DmItem = {
  channel: string;
  fromName: string;
  text: string;
  ts: number;
  fromMe: boolean;
  url?: string;
  fetchedAt: number;
  dismissedTs: number;
};

/**
 * Watched DMs that still need a reply: the newest message is theirs (`!fromMe`)
 * and the user hasn't marked it handled (`dismissedTs < ts`). Both the DM banner
 * and the day-bar attention count derive from this one predicate.
 */
export function dmsNeedingAttention(snapshot: StoreSnapshot): DmItem[] {
  return snapshot.dms.filter((d) => !d.fromMe && d.dismissedTs < d.ts);
}

/**
 * One handled request against the towles-tool MCP server, logged by the
 * dispatcher. The server runs inside the desktop app over loopback HTTP
 * (`http://127.0.0.1:8787/mcp`) — there is no CLI to start it, so an empty log
 * means no app instance is holding the port. `method` is the JSON-RPC method
 * (`initialize`,
 * `tools/call`, …); `tool` and `args` are set only for `tools/call` (args are a
 * compacted one-line rendering). `ok` is false for a JSON-RPC error or an
 * `isError` tool result, with the message in `error`. `client` is the caller's
 * `clientInfo` from the session's `initialize` (e.g. `claude-code 2.1`).
 */
export type McpCall = {
  id: number;
  ts: number;
  method: string;
  tool?: string;
  args?: string;
  ok: boolean;
  error?: string;
  durationMs?: number;
  client?: string;
};

/** The snapshot exactly as the backend sends it — see {@link WireCalEvent}. */
export type WireStoreSnapshot = Omit<StoreSnapshot, "events"> & { events: WireCalEvent[] };

/**
 * Turn a backend snapshot into the shape the screens use: the only place event
 * times are parsed, so no consumer has to think about the wire format.
 */
export function toStoreSnapshot(wire: WireStoreSnapshot): StoreSnapshot {
  return { ...wire, events: toCalEvents(wire.events) };
}

export type StoreSnapshot = {
  events: CalEvent[];
  tasks: TaskItem[];
  issues: IssueItem[];
  prs: PrItem[];
  runs: CollectRun[];
  dms: DmItem[];
  mcpCalls: McpCall[];
};

const MINUTE = 60_000;

/** An empty snapshot — the state until the real store answers. */
export const EMPTY_SNAPSHOT: StoreSnapshot = {
  events: [],
  tasks: [],
  issues: [],
  prs: [],
  runs: [],
  dms: [],
  mcpCalls: [],
};

/** Epoch ms → the RFC 3339 wire shape the calendar rows are authored in. */
function at(ms: number): string {
  return new Date(ms).toISOString();
}

/**
 * Static mock snapshot for plain-Vite browser dev (no Tauri host), so screens
 * like Cockpit render representative rows — including one PR per checks state
 * (`passing | failing | pending | none`) — instead of empty panels. Timestamps
 * are relative to load time; `live` stays false so the "not connected" banner
 * still shows.
 */
export function mockSnapshot(now: number = Date.now()): StoreSnapshot {
  // Authored in the wire shape and parsed by the real `toCalEvents`, so browser
  // dev exercises the same conversion the app does rather than a parallel one
  // that could drift from it.
  return {
    events: toCalEvents([
      {
        id: 1,
        source: "outlook",
        externalId: "mock-standup",
        title: "Team standup",
        start: at(now + 25 * MINUTE),
        end: at(now + 40 * MINUTE),
        attendees: [],
        location: "Meet",
        joinUrl: "https://meet.example.com/mock-standup",
      },
      {
        id: 2,
        source: "outlook",
        externalId: "mock-design-review",
        title: "Design review",
        start: at(now + 90 * MINUTE),
        end: at(now + 120 * MINUTE),
        attendees: [],
        location: "Meet",
      },
      {
        id: 3,
        source: "outlook",
        externalId: "mock-1on1",
        title: "1:1 with Sam",
        start: at(now + 150 * MINUTE),
        end: at(now + 180 * MINUTE),
        attendees: [],
      },
      {
        id: 4,
        source: "google",
        externalId: "mock-lunch",
        title: "Lunch & learn",
        start: at(now + 210 * MINUTE),
        end: at(now + 240 * MINUTE),
        attendees: [],
      },
      {
        id: 5,
        source: "outlook",
        externalId: "mock-planning",
        title: "Sprint planning",
        start: at(now + 270 * MINUTE),
        end: at(now + 330 * MINUTE),
        attendees: [],
      },
      {
        id: 6,
        source: "outlook",
        externalId: "mock-retro",
        title: "Retro",
        start: at(now + 360 * MINUTE),
        end: at(now + 390 * MINUTE),
        attendees: [],
      },
    ]),
    tasks: [],
    issues: [
      {
        repo: "octo/widgets",
        number: 118,
        title: "Flaky terminal resize on hidden panes",
        labels: ["bug"],
        state: "open",
        url: "https://github.com/octo/widgets/issues/118",
        updatedTs: now - 5 * 60 * MINUTE,
      },
    ],
    prs: [
      {
        repo: "octo/widgets",
        number: 42,
        title: "feat: add treemap rendering",
        branch: "feat/treemap",
        state: "open",
        checks: "passing",
        reviewState: "",
        url: "https://github.com/octo/widgets/pull/42",
        updatedTs: now - 30 * MINUTE,
      },
      {
        repo: "octo/widgets",
        number: 43,
        title: "fix: race in collector scheduler",
        branch: "fix/scheduler-race",
        state: "open",
        checks: "failing",
        reviewState: "review_requested",
        url: "https://github.com/octo/widgets/pull/43",
        updatedTs: now - 2 * 60 * MINUTE,
      },
      {
        repo: "octo/gizmos",
        number: 7,
        title: "chore: bump toolchain",
        branch: "chore/toolchain",
        state: "open",
        checks: "pending",
        reviewState: "",
        url: "https://github.com/octo/gizmos/pull/7",
        updatedTs: now - 10 * MINUTE,
      },
      {
        repo: "octo/gizmos",
        number: 8,
        title: "docs: attribution notes",
        branch: "docs/attribution",
        state: "open",
        checks: "none",
        reviewState: "",
        url: "https://github.com/octo/gizmos/pull/8",
        updatedTs: now - 26 * 60 * MINUTE,
      },
      {
        repo: "octo/gizmos",
        number: 6,
        title: "feat: task port picker",
        branch: "feat/task-ports",
        state: "merged",
        checks: "passing",
        reviewState: "",
        url: "https://github.com/octo/gizmos/pull/6",
        updatedTs: now - 45 * MINUTE,
      },
    ],
    runs: [],
    dms: [],
    mcpCalls: [
      {
        id: 4,
        ts: now - 12 * 1000,
        method: "tools/call",
        tool: "task_list",
        args: "{}",
        ok: true,
        durationMs: 6,
        client: "claude-code 2.1",
      },
      {
        id: 3,
        ts: now - 40 * 1000,
        method: "tools/call",
        tool: "task_status",
        args: '{"id":2}',
        ok: true,
        durationMs: 3,
        client: "claude-code 2.1",
      },
      {
        id: 2,
        ts: now - 55 * 1000,
        method: "tools/call",
        tool: "task_create",
        args: '{"repo":"gizmos","title":"Wire the MCP screen"}',
        ok: false,
        error: "task_create is disabled: tt-mcp's mutating tools are off until you opt in.",
        durationMs: 1,
        client: "claude-code 2.1",
      },
      {
        id: 1,
        ts: now - 2 * MINUTE,
        method: "initialize",
        ok: true,
        durationMs: 0,
        client: "claude-code 2.1",
      },
    ],
  };
}

/**
 * Subscribe to the live store snapshot: pull the initial one via
 * `store_snapshot`, then track `store://snapshot` events. Until the real store
 * answers, the snapshot is empty and `live` is false; outside Tauri entirely
 * (plain-Vite browser dev) it falls back to {@link mockSnapshot}.
 */
export function useStoreSnapshot(): { snapshot: StoreSnapshot; live: boolean } {
  const [snapshot, setSnapshot] = useState<StoreSnapshot>(EMPTY_SNAPSHOT);
  const [live, setLive] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    if (!isTauri()) {
      // Browser dev: render mock rows so screens are visually workable.
      setSnapshot(mockSnapshot());
      return;
    }
    // A `store://snapshot` event can beat the initial `store_snapshot` invoke;
    // once one has, its data is fresher, so don't let the invoke roll it back.
    let eventArrived = false;

    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");

        const sub = await listen<WireStoreSnapshot>("store://snapshot", (e) => {
          eventArrived = true;
          setSnapshot(toStoreSnapshot(e.payload));
          setLive(true);
        });
        if (disposed) {
          sub();
          return;
        }
        unlisten = sub;
      } catch {
        // Event bridge not ready — stay on the empty snapshot.
        return;
      }

      const initial = await invoke<WireStoreSnapshot>("store_snapshot");
      if (initial.isOk() && !disposed && !eventArrived) {
        setSnapshot(toStoreSnapshot(initial.value));
        setLive(true);
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return { snapshot, live };
}

/** `2:30 PM` — wall-clock time for an epoch-ms timestamp. */
export function fmtClock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {
    hour: "numeric",
    minute: "2-digit",
  });
}

/** Below this span the countdown switches to `m:ss` and (in the Cockpit) ticks
 * every second, so the final approach reads "1:30 … 0:59 … 0:05" instead of a
 * coarse "1m" that the 15s shared clock can leave stale 20s out. */
export const COUNTDOWN_SECONDS_THRESHOLD = 2 * MINUTE;

/** `0:59` / `1:30` (under {@link COUNTDOWN_SECONDS_THRESHOLD}) / `22m` /
 * `1h 05m` — a positive duration; `now` for anything non-positive. */
export function fmtCountdown(msUntil: number): string {
  if (msUntil <= 0) return "now";
  if (msUntil < COUNTDOWN_SECONDS_THRESHOLD) {
    const secs = Math.ceil(msUntil / 1000);
    const m = Math.floor(secs / 60);
    const s = secs % 60;
    return `${m}:${String(s).padStart(2, "0")}`;
  }
  const mins = Math.round(msUntil / MINUTE);
  if (mins < 60) return `${mins}m`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return `${h}h ${String(m).padStart(2, "0")}m`;
}

/**
 * Whether an event is live — started but not yet ended (`startTs <= now < endTs`).
 * An event with no `endTs` has no live window and is never live.
 */
export function eventIsLive(e: CalEvent, now: number): boolean {
  return e.startTs <= now && e.endTs !== undefined && now < e.endTs;
}

/**
 * The meeting to surface on the Cockpit strip: the one in progress right now,
 * or the soonest still to start — whichever begins first. Mirrors tt-store's
 * `current_or_next_event`, so an in-progress meeting stays visible instead of
 * vanishing the instant it starts. An event with no `endTs` is a point in time,
 * shown only up to its start.
 */
export function currentOrNextEvent(events: CalEvent[], now: number): CalEvent | undefined {
  return events
    .filter((e) => (e.endTs !== undefined ? now < e.endTs : e.startTs >= now))
    .toSorted((a, b) => a.startTs - b.startTs)[0];
}

/** Whole minutes from `now` until `ts` (negative when `ts` is in the past). */
export function minutesUntil(ts: number, now: number): number {
  return Math.round((ts - now) / MINUTE);
}

/** `just now` / `12m ago` / `3h ago` / `2d ago` — coarse relative age. */
export function fmtAge(ms: number, now: number): string {
  const diff = now - ms;
  if (diff < MINUTE) return "just now";
  const mins = Math.round(diff / MINUTE);
  if (mins < 60) return `${mins}m ago`;
  const h = Math.round(mins / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.round(h / 24)}d ago`;
}

/**
 * Name of the checkout/task this window is running (e.g. `towles-tool-rs-task-2`),
 * from the Rust `app_task` command. `null` outside Tauri (plain-Vite browser dev)
 * so the header badge is hidden there. Lets several tasks' windows be told apart.
 */
export function useAppTask(): string | null {
  const [task, setTask] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void (async () => {
      const s = await invoke<string>("app_task");
      if (active) setTask(s.unwrapOr(null));
    })();
    return () => {
      active = false;
    };
  }, []);
  return task;
}

/** Create a task; resolves to its id. `status` defaults to Backlog backend-side. */
export const storeAddTask = (text: string, opts?: { status?: TaskStatus }) =>
  invoke<number>("store_add_task", { text, status: opts?.status });

/** Move a task to another board column (appended at the end of it). */
export const storeSetTaskStatus = (id: number, status: TaskStatus) =>
  invoke<void>("store_set_task_status", { id, status });

/** Move a task to `status` at task `index` within that column (drag-to-reorder). */
export const storeSetTaskPosition = (id: number, status: TaskStatus, index: number) =>
  invoke<void>("store_set_task_position", { id, status, index });

/** Overwrite a task's editable fields. */
export const storeUpdateTask = (id: number, text: string, notes?: string) =>
  invoke<void>("store_update_task", { id, text, notes });

/** Delete a task outright. */
export const storeDeleteTask = (id: number) => invoke<void>("store_delete_task", { id });

/** Sweep Done tasks older than the backend's retention window (default 7 days). */
export const storeClearDone = () => invoke<void>("store_clear_done");

/** Open a GitHub issue in `repo` for an existing task and attach the two. */
export const storePromoteTaskToIssue = (id: number, repo: string) =>
  invoke<void>("store_promote_task_to_issue", { id, repo });

/** Attach a GitHub issue to a task. */
export const storeAttachTaskIssue = (id: number, repo: string, number: number, url: string) =>
  invoke<void>("store_attach_task_issue", { id, repo, number, url });

/** Detach a GitHub issue from a task. */
export const storeDetachTaskIssue = (id: number, repo: string, number: number) =>
  invoke<void>("store_detach_task_issue", { id, repo, number });

/** Attach a GitHub PR to a task (worktree-branch PRs auto-attach on collect). */
export const storeAttachTaskPr = (id: number, repo: string, number: number, url: string) =>
  invoke<void>("store_attach_task_pr", { id, repo, number, url });

/** Detach a GitHub PR from a task. */
export const storeDetachTaskPr = (id: number, repo: string, number: number) =>
  invoke<void>("store_detach_task_pr", { id, repo, number });

/** Bind a task to its repo, and to the worktree its work happens in once
 * one exists. The new-task flow calls this at submit with the repo alone, then
 * again with `branch`/`dir` once `task_create` resolves. */
export const storeTaskSetWorktree = (
  id: number,
  repoRoot: string,
  branch: string | undefined,
  opts?: { repo?: string; dir?: string },
) =>
  invoke<void>("store_task_set_worktree", {
    id,
    repoRoot,
    branch,
    repo: opts?.repo,
    dir: opts?.dir,
  });

/** Open issues in `dir`'s repo, for the new-task flow's issue picker. */
export const storeGhIssuesList = (dir: string, assignedToMe: boolean) =>
  invoke<IssueItem[]>("store_gh_issues_list", { dir, assignedToMe });

/** Mark a watched Slack DM handled up to `ts`, clearing its banner. */
export const storeDmDismiss = (channel: string, ts: number) =>
  invoke<void>("store_dm_dismiss", { channel, ts });

/** Append a line to today's journal note. */
export const journalLog = (text: string) => invoke<void>("journal_log", { text });

/**
 * Force the issues, PRs, and (when configured) Slack collectors to run right
 * now, bypassing the scheduler cadence — calendar is intentionally excluded
 * (it spends claude tokens). The store snapshot re-emits from Rust when the run
 * finishes.
 *
 * The `boolean` is a domain answer, not a success flag: `true` when this call
 * kicked off a refresh, `false` when one was already in flight (an
 * overlap-guarded no-op). A failed or unavailable command is the `Err` side.
 */
export async function storeCollectNow(): Promise<Result<boolean, IpcError>> {
  const result = await invoke<{ started: boolean }>("store_collect_now");
  return result.map((r) => r.started);
}
