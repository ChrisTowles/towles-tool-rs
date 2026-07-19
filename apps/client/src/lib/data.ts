import { useEffect, useState } from "react";
import type { Result } from "better-result";
import type { IpcError } from "./errors";
import { invoke, isTauri } from "./tauri";

/**
 * Client-side view of the personal-HQ store (the Rust `tt-store` crate, surfaced
 * by the Tauri app). Mirrors the serialized snapshot (camelCase) that the
 * `store_snapshot` command returns and the `store://snapshot` event broadcasts.
 * Timestamps are epoch milliseconds. Before the store answers the hook holds an
 * empty snapshot with `live: false`; outside Tauri (plain-Vite browser dev) it
 * holds {@link mockSnapshot} instead, still with `live: false`.
 */

export type CalEvent = {
  id: number;
  externalId: string;
  title: string;
  startTs: number;
  endTs?: number;
  attendees: string[];
  location?: string;
  joinUrl?: string;
};

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

export type TaskItem = {
  id: number;
  text: string;
  status: TaskStatus;
  position: number;
  dueTs?: number;
  /** Set once the todo is promoted to / linked with a GitHub issue. */
  repo?: string;
  issueNumber?: number;
  issueUrl?: string;
  createdAt: number;
  completedAt?: number;
  /** Free-form context attached to the todo. */
  notes?: string;
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
 * One handled request against the towles-tool MCP server (`tt mcp serve`),
 * logged by the dispatcher. `method` is the JSON-RPC method (`initialize`,
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

/**
 * Static mock snapshot for plain-Vite browser dev (no Tauri host), so screens
 * like Cockpit render representative rows — including one PR per checks state
 * (`passing | failing | pending | none`) — instead of empty panels. Timestamps
 * are relative to load time; `live` stays false so the "not connected" banner
 * still shows.
 */
export function mockSnapshot(now: number = Date.now()): StoreSnapshot {
  return {
    events: [
      {
        id: 1,
        externalId: "mock-standup",
        title: "Team standup",
        startTs: now + 25 * MINUTE,
        endTs: now + 40 * MINUTE,
        attendees: [],
        location: "Meet",
        joinUrl: "https://meet.example.com/mock-standup",
      },
      {
        id: 2,
        externalId: "mock-design-review",
        title: "Design review",
        startTs: now + 90 * MINUTE,
        endTs: now + 120 * MINUTE,
        attendees: [],
        location: "Meet",
      },
      {
        id: 3,
        externalId: "mock-1on1",
        title: "1:1 with Sam",
        startTs: now + 150 * MINUTE,
        endTs: now + 180 * MINUTE,
        attendees: [],
      },
      {
        id: 4,
        externalId: "mock-lunch",
        title: "Lunch & learn",
        startTs: now + 210 * MINUTE,
        endTs: now + 240 * MINUTE,
        attendees: [],
      },
      {
        id: 5,
        externalId: "mock-planning",
        title: "Sprint planning",
        startTs: now + 270 * MINUTE,
        endTs: now + 330 * MINUTE,
        attendees: [],
      },
      {
        id: 6,
        externalId: "mock-retro",
        title: "Retro",
        startTs: now + 360 * MINUTE,
        endTs: now + 390 * MINUTE,
        attendees: [],
      },
    ],
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
        title: "feat: slot port picker",
        branch: "feat/slot-ports",
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
        tool: "day_brief",
        args: "{}",
        ok: true,
        durationMs: 6,
        client: "claude-code 2.1",
      },
      {
        id: 3,
        ts: now - 40 * 1000,
        method: "tools/call",
        tool: "tasks_open",
        args: "{}",
        ok: true,
        durationMs: 3,
        client: "claude-code 2.1",
      },
      {
        id: 2,
        ts: now - 55 * 1000,
        method: "tools/call",
        tool: "todo_create",
        args: '{"title":"Wire the MCP screen"}',
        ok: false,
        error: "unknown tool: todo_create",
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

        const sub = await listen<StoreSnapshot>("store://snapshot", (e) => {
          eventArrived = true;
          setSnapshot(e.payload);
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

      const initial = await invoke<StoreSnapshot>("store_snapshot");
      if (initial.isOk() && !disposed && !eventArrived) {
        setSnapshot(initial.value);
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

/** `Jul 15` — calendar day for an epoch-ms timestamp (used for due dates). */
export function fmtDay(ms: number): string {
  return new Date(ms).toLocaleDateString([], {
    month: "short",
    day: "numeric",
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
 * Name of the checkout/slot this window is running (e.g. `towles-tool-rs-slot-2`),
 * from the Rust `app_slot` command. `null` outside Tauri (plain-Vite browser dev)
 * so the header badge is hidden there. Lets several slots' windows be told apart.
 */
export function useAppSlot(): string | null {
  const [slot, setSlot] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    void (async () => {
      const s = await invoke<string>("app_slot");
      if (active) setSlot(s.unwrapOr(null));
    })();
    return () => {
      active = false;
    };
  }, []);
  return slot;
}

/** Create a todo in Backlog. */
export const storeAddTask = (text: string, dueTs?: number, repo?: string) =>
  invoke<void>("store_add_task", { text, dueTs, repo });

/** Move a todo to another kanban column (appended at the end of it). */
export const storeSetTaskStatus = (id: number, status: TaskStatus) =>
  invoke<void>("store_set_task_status", { id, status });

/** Move a todo to `status` at slot `index` within that column (drag-to-reorder). */
export const storeSetTaskPosition = (id: number, status: TaskStatus, index: number) =>
  invoke<void>("store_set_task_position", { id, status, index });

/** Overwrite a todo's editable fields. */
export const storeUpdateTask = (id: number, text: string, notes?: string, dueTs?: number) =>
  invoke<void>("store_update_task", { id, text, notes, dueTs });

/** Delete a todo outright. */
export const storeDeleteTask = (id: number) => invoke<void>("store_delete_task", { id });

/** Sweep Done todos older than the backend's retention window (default 7 days). */
export const storeClearDone = () => invoke<void>("store_clear_done");

/** Open a GitHub issue in `repo` for an existing todo and link the two. */
export const storePromoteTaskToIssue = (id: number, repo: string) =>
  invoke<void>("store_promote_task_to_issue", { id, repo });

/** One Agentboard-tracked repo, resolved to its GitHub `owner/name`. */
export type GhRepoOption = { dir: string; name: string };

/** Tracked repos resolved to their GitHub identity, for the "Import from
 * GitHub" dialog's repo picker. The failure stays in the `Result` (rather than
 * degrading to an empty list) so the dialog can show a real error state. */
export const storeGhTrackedRepos = () => invoke<GhRepoOption[]>("store_gh_tracked_repos");

/** Open issues in `dir`'s repo, for the import dialog's issue picker. */
export const storeGhIssuesList = (dir: string, assignedToMe: boolean, milestone?: string) =>
  invoke<IssueItem[]>("store_gh_issues_list", { dir, assignedToMe, milestone });

/** Open milestone titles in `dir`'s repo, for the import dialog's filter. */
export const storeGhMilestonesList = (dir: string) =>
  invoke<string[]>("store_gh_milestones_list", { dir });

/** One issue selected in the import dialog. */
export type ImportIssueInput = { repo: string; number: number; title: string; url: string };

/** Import selected GitHub issues onto the board as new Backlog todos.
 * Resolves to how many todos were created (issues already linked are
 * skipped). */
export const storeImportIssues = (items: ImportIssueInput[]) =>
  invoke<number>("store_import_issues", { items });

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
