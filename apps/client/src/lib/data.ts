import { useEffect, useState } from "react";
import { toast } from "sonner";

/**
 * Client-side view of the personal-HQ store (the Rust `tt-store` crate, surfaced
 * by the Tauri app). Mirrors the serialized snapshot (camelCase) that the
 * `store_snapshot` command returns and the `store://snapshot` event broadcasts.
 * Timestamps are epoch milliseconds. Outside Tauri the hook falls back to
 * `MOCK_SNAPSHOT` (shaped like the real collector output) with `live: false`.
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

export const isTauri = () => "__TAURI_INTERNALS__" in window;

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/** Rich fake snapshot, computed at module load so ages/countdowns read live. */
function buildMockSnapshot(): StoreSnapshot {
  const now = Date.now();

  return {
    events: [
      {
        id: 1,
        externalId: "cal-standup",
        title: "Team standup",
        startTs: now - 40 * MINUTE,
        endTs: now - 25 * MINUTE,
        attendees: ["Chris", "Priya", "Marcus"],
        location: "Zoom",
        joinUrl: "https://example.com/j/standup",
      },
      {
        id: 2,
        externalId: "cal-platform-sync",
        title: "Platform sync",
        startTs: now + 12 * MINUTE,
        endTs: now + 12 * MINUTE + 30 * MINUTE,
        attendees: ["Chris", "Dana K.", "Marcus", "Lee"],
        location: "Meet",
        joinUrl: "https://example.com/j/platform-sync",
      },
      {
        id: 3,
        externalId: "cal-1on1",
        title: "1:1 with Dana",
        startTs: now + 3 * HOUR,
        endTs: now + 3 * HOUR + 30 * MINUTE,
        attendees: ["Chris", "Dana K."],
        location: "Zoom",
        joinUrl: "https://example.com/j/1on1",
      },
    ],
    tasks: [
      {
        id: 1,
        text: "Refunds double-charge on retry",
        status: "doing",
        position: 0,
        repo: "w/acme-billing",
        issueNumber: 390,
        issueUrl: "https://github.com/w/acme-billing/issues/390",
        createdAt: now - 5 * HOUR,
      },
      {
        id: 2,
        text: "Kanban backed by issues",
        status: "next",
        position: 0,
        repo: "p/towles-tool",
        issueNumber: 61,
        issueUrl: "https://github.com/p/towles-tool/issues/61",
        createdAt: now - 3 * HOUR,
      },
      {
        id: 3,
        text: "Draft platform-sync talking points",
        status: "backlog",
        position: 0,
        dueTs: now + 15 * MINUTE,
        createdAt: now - 90 * MINUTE,
      },
      {
        id: 4,
        text: "a11y: focus traps in modal",
        status: "backlog",
        position: 1,
        repo: "w/acme-web",
        issueNumber: 255,
        issueUrl: "https://github.com/w/acme-web/issues/255",
        createdAt: now - 20 * HOUR,
      },
      {
        id: 5,
        text: "Split zsh aliases per-OS",
        status: "done",
        position: 0,
        createdAt: now - 26 * HOUR,
        completedAt: now - 2 * HOUR,
      },
    ],
    issues: [
      {
        repo: "w/acme-billing",
        number: 390,
        title: "Refunds double-charge on retry",
        labels: ["bug", "P1"],
        state: "open",
        url: "https://github.com/w/acme-billing/issues/390",
        updatedTs: now - 40 * MINUTE,
      },
      {
        repo: "p/towles-tool",
        number: 61,
        title: "Kanban backed by issues",
        labels: ["feature"],
        state: "open",
        url: "https://github.com/p/towles-tool/issues/61",
        updatedTs: now - 3 * HOUR,
      },
      {
        repo: "w/acme-web",
        number: 255,
        title: "a11y: focus traps in modal",
        labels: ["a11y"],
        state: "open",
        url: "https://github.com/w/acme-web/issues/255",
        updatedTs: now - 20 * HOUR,
      },
    ],
    prs: [
      {
        repo: "w/acme-billing",
        number: 412,
        title: "Fix invoice rounding",
        branch: "fix/invoice-rounding",
        state: "open",
        checks: "failing",
        reviewState: "changes_requested",
        url: "https://github.com/w/acme-billing/pull/412",
        updatedTs: now - 30 * MINUTE,
      },
      {
        repo: "w/acme-web",
        number: 203,
        title: "Upgrade to React 19",
        branch: "chore/react-19",
        state: "open",
        checks: "passing",
        reviewState: "review_requested",
        url: "https://github.com/w/acme-web/pull/203",
        updatedTs: now - 3 * HOUR,
      },
      {
        repo: "p/dotfiles",
        number: 88,
        title: "zsh: faster prompt",
        branch: "feat/fast-prompt",
        state: "open",
        checks: "pending",
        reviewState: "",
        url: "https://github.com/p/dotfiles/pull/88",
        updatedTs: now - 6 * HOUR,
      },
    ],
    runs: [
      { collector: "claude:calendar", ranAt: now - 12 * MINUTE, ok: true },
      { collector: "issues", ranAt: now - 4 * MINUTE, ok: true },
      { collector: "prs", ranAt: now - 1 * MINUTE, ok: true },
    ],
  };
}

export const MOCK_SNAPSHOT: StoreSnapshot = buildMockSnapshot();

/**
 * Subscribe to the live store snapshot: pull the initial one via
 * `store_snapshot`, then track `store://snapshot` events. Falls back to
 * `MOCK_SNAPSHOT` with `live: false` when not running under Tauri or the store
 * isn't ready.
 */
export function useStoreSnapshot(): { snapshot: StoreSnapshot; live: boolean } {
  const [snapshot, setSnapshot] = useState<StoreSnapshot>(MOCK_SNAPSHOT);
  const [live, setLive] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const { listen } = await import("@tauri-apps/api/event");

        const sub = await listen<StoreSnapshot>("store://snapshot", (e) => {
          setSnapshot(e.payload);
          setLive(true);
        });
        if (disposed) {
          sub();
          return;
        }
        unlisten = sub;

        const initial = await invoke<StoreSnapshot>("store_snapshot");
        if (!disposed) {
          setSnapshot(initial);
          setLive(true);
        }
      } catch {
        // Not under Tauri / store not ready — stay on MOCK_SNAPSHOT.
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
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/** `22m` / `1h 05m` — a positive duration; `now` for anything non-positive. */
export function fmtCountdown(msUntil: number): string {
  if (msUntil <= 0) return "now";
  const mins = Math.round(msUntil / MINUTE);
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
 * Run a store write command, degrading gracefully in the browser: outside Tauri
 * (or on any failure) show a toast and report failure so callers can revert an
 * optimistic update.
 */
async function storeInvoke(command: string, args: Record<string, unknown>): Promise<boolean> {
  if (!isTauri()) {
    toast.info("not wired in browser");
    return false;
  }
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke(command, args);
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  }
}

export const storeAddTask = (text: string, dueTs?: number) =>
  storeInvoke("store_add_task", { text, dueTs });

export const storeSetTaskStatus = (id: number, status: TaskStatus) =>
  storeInvoke("store_set_task_status", { id, status });

export const storePromoteTaskToIssue = (id: number, repo: string) =>
  storeInvoke("store_promote_task_to_issue", { id, repo });

export const journalLog = (text: string) => storeInvoke("journal_log", { text });
