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

export type TaskItem = {
  id: number;
  source: string;
  sourceRef?: string;
  text: string;
  dueTs?: number;
  done: boolean;
  createdAt: number;
  completedAt?: number;
};

export type EmailItem = {
  id: number;
  externalId: string;
  fromName: string;
  fromAddr: string;
  subject: string;
  summary: string;
  tag: "needs_reply" | "invite" | "fyi";
  receivedTs: number;
  archived: boolean;
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
  emails: EmailItem[];
  prs: PrItem[];
  runs: CollectRun[];
};

export const isTauri = () => "__TAURI_INTERNALS__" in window;

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;

/** Rich fake snapshot, computed at module load so ages/countdowns read live. */
function buildMockSnapshot(): StoreSnapshot {
  const now = Date.now();
  const startOfTomorrow = new Date(now);
  startOfTomorrow.setDate(startOfTomorrow.getDate() + 1);
  startOfTomorrow.setHours(9, 30, 0, 0);

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
        startTs: now + 22 * MINUTE,
        endTs: now + 22 * MINUTE + 30 * MINUTE,
        attendees: ["Chris", "Dana K.", "Marcus", "Lee"],
        location: "Meet",
        joinUrl: "https://example.com/j/platform-sync",
      },
      {
        id: 3,
        externalId: "cal-design-review",
        title: "Design review: session storage",
        startTs: now + 2 * HOUR + 10 * MINUTE,
        endTs: now + 2 * HOUR + 55 * MINUTE,
        attendees: ["Chris", "Sam", "Dana K."],
        location: "Meet",
        joinUrl: "https://example.com/j/design-review",
      },
      {
        id: 4,
        externalId: "cal-1on1",
        title: "1:1 with Priya",
        startTs: now + 4 * HOUR,
        endTs: now + 4 * HOUR + 30 * MINUTE,
        attendees: ["Chris", "Priya"],
      },
      {
        id: 5,
        externalId: "cal-oncall",
        title: "On-call handoff",
        startTs: startOfTomorrow.getTime(),
        endTs: startOfTomorrow.getTime() + 30 * MINUTE,
        attendees: ["Chris", "Priya"],
      },
    ],
    tasks: [
      {
        id: 1,
        source: "manual",
        text: "Draft platform-sync talking points",
        dueTs: now + 15 * MINUTE,
        done: false,
        createdAt: now - 3 * HOUR,
      },
      {
        id: 2,
        source: "github",
        sourceRef: "ChrisTowles/towles-tool-rs#4",
        text: "Fix failing checks on feat/app-shell",
        dueTs: now + 3 * HOUR,
        done: false,
        createdAt: now - 5 * HOUR,
      },
      {
        id: 3,
        source: "manual",
        text: "Reply to Dana about the agenda",
        done: false,
        createdAt: now - 90 * MINUTE,
      },
      {
        id: 4,
        source: "email",
        sourceRef: "invite-brownbag",
        text: "Decide on Friday brown-bag slot",
        dueTs: now + 26 * HOUR,
        done: false,
        createdAt: now - 20 * HOUR,
      },
      {
        id: 5,
        source: "manual",
        text: "Merge the Tailwind v4 cutover",
        done: true,
        createdAt: now - 26 * HOUR,
        completedAt: now - 2 * HOUR,
      },
    ],
    emails: [
      {
        id: 1,
        externalId: "mail-dana-agenda",
        fromName: "Dana K.",
        fromAddr: "dana@example.com",
        subject: "Agenda for today's platform sync",
        summary: "Wants your take on the session-storage rollout before we meet.",
        tag: "needs_reply",
        receivedTs: now - 35 * MINUTE,
        archived: false,
      },
      {
        id: 2,
        externalId: "mail-gh-issue",
        fromName: "GitHub",
        fromAddr: "notifications@github.com",
        subject: "[towles-tool-rs] Terminal panes leak PTYs on close (#12)",
        summary: "New issue assigned to you; repro attached, needs triage.",
        tag: "needs_reply",
        receivedTs: now - 2 * HOUR,
        archived: false,
      },
      {
        id: 3,
        externalId: "mail-brownbag",
        fromName: "Sam Ortiz",
        fromAddr: "sam@example.com",
        subject: "Invite: Friday brown-bag on Tauri",
        summary: "Proposes 12:00 Friday; asks if that works for you to present.",
        tag: "invite",
        receivedTs: now - 5 * HOUR,
        archived: false,
      },
      {
        id: 4,
        externalId: "mail-digest",
        fromName: "Rust Weekly",
        fromAddr: "digest@this-week-in-rust.org",
        subject: "This Week in Rust 601",
        summary: "Cargo workspace feature-unification RFC lands; async gen notes.",
        tag: "fyi",
        receivedTs: now - 7 * HOUR,
        archived: false,
      },
      {
        id: 5,
        externalId: "mail-ci",
        fromName: "CI",
        fromAddr: "ci@example.com",
        subject: "Nightly build succeeded",
        summary: "All targets green on main; artifacts uploaded.",
        tag: "fyi",
        receivedTs: now - 9 * HOUR,
        archived: false,
      },
    ],
    prs: [
      {
        repo: "ChrisTowles/towles-tool-rs",
        number: 4,
        title: "feat: Yaak-style app shell for the desktop UI",
        branch: "feat/app-shell",
        state: "open",
        checks: "failing",
        reviewState: "changes_requested",
        url: "https://github.com/ChrisTowles/towles-tool-rs/pull/4",
        updatedTs: now - 30 * MINUTE,
      },
      {
        repo: "ChrisTowles/towles-tool-rs",
        number: 3,
        title: "feat: agentboard tmux mode follow-ups",
        branch: "feat/agentboard-tmux-2",
        state: "open",
        checks: "passing",
        reviewState: "approved",
        url: "https://github.com/ChrisTowles/towles-tool-rs/pull/3",
        updatedTs: now - 3 * HOUR,
      },
      {
        repo: "ChrisTowles/dotfiles",
        number: 88,
        title: "chore: sync zsh plugin pins",
        branch: "chore/zsh-pins",
        state: "open",
        checks: "passing",
        reviewState: "review_requested",
        url: "https://github.com/ChrisTowles/dotfiles/pull/88",
        updatedTs: now - 6 * HOUR,
      },
    ],
    runs: [
      { collector: "claude:email", ranAt: now - 12 * MINUTE, ok: true },
      { collector: "claude:calendar", ranAt: now - 12 * MINUTE, ok: true },
      { collector: "claude:tasks", ranAt: now - 12 * MINUTE, ok: true },
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
  } catch {
    toast.info("not wired in browser");
    return false;
  }
}

export const storeAddTask = (text: string, dueTs?: number) =>
  storeInvoke("store_add_task", { text, dueTs });

export const storeSetTaskDone = (id: number, done: boolean) =>
  storeInvoke("store_set_task_done", { id, done });

export const storeArchiveEmail = (id: number) => storeInvoke("store_archive_email", { id });

export const journalLog = (text: string) => storeInvoke("journal_log", { text });
