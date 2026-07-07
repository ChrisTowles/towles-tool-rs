import { useEffect, useState } from "react";
import { invokeOk, isTauri } from "./tauri";

/**
 * Client-side view of the personal-HQ store (the Rust `tt-store` crate, surfaced
 * by the Tauri app). Mirrors the serialized snapshot (camelCase) that the
 * `store_snapshot` command returns and the `store://snapshot` event broadcasts.
 * Timestamps are epoch milliseconds. Before the store answers (or outside Tauri)
 * the hook holds an empty snapshot with `live: false`.
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

export type StoreSnapshot = {
  events: CalEvent[];
  tasks: TaskItem[];
  issues: IssueItem[];
  prs: PrItem[];
  runs: CollectRun[];
};

const MINUTE = 60_000;

/** An empty snapshot — the state until the real store answers. */
export const EMPTY_SNAPSHOT: StoreSnapshot = {
  events: [],
  tasks: [],
  issues: [],
  prs: [],
  runs: [],
};

/**
 * Subscribe to the live store snapshot: pull the initial one via
 * `store_snapshot`, then track `store://snapshot` events. Until the real store
 * answers (or outside Tauri), the snapshot is empty and `live` is false.
 */
export function useStoreSnapshot(): { snapshot: StoreSnapshot; live: boolean } {
  const [snapshot, setSnapshot] = useState<StoreSnapshot>(EMPTY_SNAPSHOT);
  const [live, setLive] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
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

export const journalLog = (text: string) => invokeOk("journal_log", { text });
