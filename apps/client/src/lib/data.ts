import { useEffect, useState } from "react";
import { invokeOk, isTauri } from "./tauri";

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
export const TASK_STATUSES = [
  "backlog",
  "next",
  "doing",
  "review",
  "done",
] as const;
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

export type StoreSnapshot = {
  events: CalEvent[];
  tasks: TaskItem[];
  issues: IssueItem[];
  prs: PrItem[];
  runs: CollectRun[];
  dms: DmItem[];
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
    ],
    runs: [],
    dms: [],
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
        const { invoke } = await import("@tauri-apps/api/core");
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

        const initial = await invoke<StoreSnapshot>("store_snapshot");
        if (!disposed && !eventArrived) {
          setSnapshot(initial);
          setLive(true);
        }
      } catch {
        // Not under Tauri / store not ready — stay on the empty snapshot.
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

/** `<1m` / `22m` / `1h 05m` — a positive duration; `now` for anything
 * non-positive. Sub-minute positive spans render `<1m` instead of rounding to
 * `0m`, so a meeting 20s out reads "in <1m", never "in 0m". */
export function fmtCountdown(msUntil: number): string {
  if (msUntil <= 0) return "now";
  const mins = Math.round(msUntil / MINUTE);
  if (mins < 1) return "<1m";
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
    .sort((a, b) => a.startTs - b.startTs)[0];
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
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const s = await invoke<string>("app_slot");
        if (active) setSlot(s);
      } catch {
        /* leave null — badge stays hidden */
      }
    })();
    return () => {
      active = false;
    };
  }, []);
  return slot;
}

export const storeAddTask = (text: string, dueTs?: number) =>
  invokeOk("store_add_task", { text, dueTs });

export const storeSetTaskStatus = (id: number, status: TaskStatus) =>
  invokeOk("store_set_task_status", { id, status });

export const storePromoteTaskToIssue = (id: number, repo: string) =>
  invokeOk("store_promote_task_to_issue", { id, repo });

export const storeDmDismiss = (channel: string, ts: number) =>
  invokeOk("store_dm_dismiss", { channel, ts });

export const journalLog = (text: string) => invokeOk("journal_log", { text });
