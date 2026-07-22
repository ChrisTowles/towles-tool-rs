import { useCallback, useEffect, useRef, useState } from "react";
import { UserSettingsSchema } from "./schemas/settings";
import { invoke } from "./tauri";
import { slugify } from "./slug";

/**
 * Client-side view of the shared user settings (`crates/tt-config`), read/written
 * over the `settings_get` / `settings_set` Tauri commands. Field names mirror the
 * serialized camelCase model. The `agentboard` block is TS-owned and opaque here —
 * carried through untouched so a save never drops it.
 */

export type JournalSettings = {
  baseFolder: string;
  dailyPathTemplate: string;
  meetingPathTemplate: string;
  notePathTemplate: string;
  templateDir: string;
};

/** Working-hours gate for the calendar collector (weekdays: 0 = Monday … 6 = Sunday). */
export type CalendarQuietHours = {
  enabled: boolean;
  startHour: number;
  endHour: number;
  weekdays: number[];
};

/**
 * One calendar the collector pulls, with the `claude -p` prompt it uses. `id` is
 * the store lane a pull replaces, so it must stay stable; the prompt is
 * user-editable on purpose — the built-in defaults drive a Google/Outlook MCP,
 * but a machine without those can point the source at whatever does work there
 * (a CLI, a script, a different MCP), as long as it still answers with the same
 * JSON array.
 */
export type CalendarSource = {
  id: string;
  label: string;
  enabled: boolean;
  prompt: string;
};

/**
 * A store-lane id for a newly added calendar, unique among `sources`.
 *
 * Slugged from the generated label (`Calendar 3`) so the id stays readable in
 * the store, then suffixed until it's free — a source can be removed and
 * re-added, so the label's own number is no guarantee of uniqueness. Ids are
 * assigned once and never edited afterwards: the id names the lane a pull
 * replaces, so changing it would orphan every row already stored under the old
 * one.
 */
export function nextCalendarSourceId(sources: CalendarSource[], label: string): string {
  const taken = new Set(sources.map((s) => s.id));
  // The shared slug rule, not a second regex: this one produces a *permanent*
  // store-lane key, so a variant that leaves a trailing `-` (as a hand-rolled
  // `[^a-z0-9]+` does) bakes the difference into the database.
  const base = slugify(label) || "calendar";
  if (!taken.has(base)) return base;
  for (let n = 2; ; n += 1) {
    const candidate = `${base}-${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/**
 * A **prompt improver**: one button in the new-task form that rewrites the goal
 * you typed before the task starts (Direct / Plan / Brainstorm by default).
 *
 * Clicking one runs `claude -p` (the `task_suggest` command) with `prompt` as
 * the *instruction*, and fills the form's goal + branch fields with the result —
 * editable, with Undo. Because the improved text lands in the field, what you
 * see is what the session launches with: nothing is wrapped at launch time and
 * the `claude` CLI flags are never touched.
 *
 * `prompt` is therefore an instruction *about* the goal ("turn this into a
 * request for a plan"), not a template containing it. `preferred` decides
 * whether it gets its own button or sits under the form's "More" menu.
 */
export type PromptImprover = {
  id: string;
  label: string;
  enabled: boolean;
  preferred: boolean;
  prompt: string;
};

/**
 * A stable id for a newly added prompt improver, unique among `improvers`. Same
 * slug rule as {@link nextCalendarSourceId}: the id is a permanent key (the
 * form's last-picked choice is stored by id), so it must stay stable once
 * assigned.
 */
export function nextPromptImproverId(improvers: PromptImprover[], label: string): string {
  const taken = new Set(improvers.map((t) => t.id));
  const base = slugify(label) || "improver";
  if (!taken.has(base)) return base;
  for (let n = 2; ; n += 1) {
    const candidate = `${base}-${n}`;
    if (!taken.has(candidate)) return candidate;
  }
}

export type CalendarCollector = {
  enabled: boolean;
  refreshMinutes: number;
  quietHours: CalendarQuietHours;
  sources: CalendarSource[];
};

export type PrCollector = {
  enabled: boolean;
  refreshSeconds: number;
  mergedRefreshMinutes: number;
};

export type IssueCollector = {
  enabled: boolean;
  refreshMinutes: number;
};

export type SlackDmCollector = {
  enabled: boolean;
  token: string;
  /** Optional app-level token (xapp-…) enabling Socket Mode real-time delivery. */
  appToken: string;
  watchUserId: string;
  watchName: string;
  refreshSeconds: number;
};

export type CollectorsSettings = {
  calendar: CalendarCollector;
  prs: PrCollector;
  issues: IssueCollector;
  slack: SlackDmCollector;
};

export type UserSettings = {
  preferredEditor: string;
  journalSettings: JournalSettings;
  /** Prompt-improver templates for the new-task form. See {@link PromptImprover}. */
  promptImprovers: PromptImprover[];
  collectors: CollectorsSettings;
  /**
   * Mostly TS-owned UI block, carried through opaquely so a save never drops
   * it. The app edits these keys: `notifyNeedsYou` (desktop notification when a
   * session flips into needs-you; unset = on), `notifyMeetingStart` (fired when
   * the next meeting's countdown reaches zero; unset = on),
   * `notifyReviewRequested` (fired when a PR newly needs your review; unset =
   * on), `notifyChecksFailed` (fired when one of your PRs' CI flips to failing;
   * unset = on), `notifyStaleCollector` (fired when a collector stops
   * refreshing; unset = on), `compactRecommendPercent` (context-usage % at which
   * a session is flagged for compaction; unset = 30), `copyOnSelect` (terminal
   * copies the selection to the clipboard on selection end; unset = off),
   * `terminalFontSize` (canvas terminal font px; unset = 13), and
   * `shortcutsWorkInTerminal` (board-wide action shortcuts, e.g. jump to
   * next/prev session needing you, fire even while a terminal has focus;
   * unset = on), `boardGroupByRepo` (the Board kanban groups tasks into
   * per-repo swimlanes; unset = on), and `hideInactiveRepos` (the Agentboard
   * rail's eye-icon "hide inactive repos" filter; unset = off).
   */
  agentboard?: {
    notifyNeedsYou?: boolean;
    notifyMeetingStart?: boolean;
    notifyReviewRequested?: boolean;
    notifyChecksFailed?: boolean;
    notifyStaleCollector?: boolean;
    compactRecommendPercent?: number;
    copyOnSelect?: boolean;
    terminalFontSize?: number;
    shortcutsWorkInTerminal?: boolean;
    boardGroupByRepo?: boolean;
    hideInactiveRepos?: boolean;
  } & Record<string, unknown>;
};

export type SaveState = "idle" | "saving" | "saved" | "error";

/** Debounce applied to `update(fn, { defer: true })` — long enough that typing a
 * path template or pasting a token doesn't write a half-finished value to disk
 * (which `settings_set` would hand straight to the live scheduler), short enough
 * that pausing feels like it saved. */
const DEFER_MS = 500;

/** Fired on `window` after a successful `settings_set` — lets other in-app
 * consumers of a settings value (e.g. terminal prefs, the shortcuts registry)
 * refresh live instead of waiting for a `"focus"` event that no longer fires
 * when Settings is just a tab in the same window. */
export const SETTINGS_SAVED_EVENT = "tt:settings-saved";

/**
 * Read the shared settings file, validated against {@link UserSettingsSchema}.
 * `null` in browser dev or when the read fails — every consumer here falls back
 * to a built-in default rather than surfacing the failure, so the distinction
 * isn't worth propagating. Shared by the settings screen, terminal prefs, and
 * the shortcuts registry, which all read the same file on the same triggers.
 */
export async function loadUserSettings(): Promise<UserSettings | null> {
  const result = await invoke<UserSettings>("settings_get", {}, { schema: UserSettingsSchema });
  return result.unwrapOr(null);
}

/**
 * Persist the whole settings object and notify in-app listeners. Callers pass
 * the full object (not a patch) so the TS CLI's unknown keys survive the save.
 */
export async function saveUserSettings(settings: UserSettings): Promise<boolean> {
  const saved = await invoke("settings_set", { settings });
  if (saved.isOk()) window.dispatchEvent(new Event(SETTINGS_SAVED_EVENT));
  return saved.isOk();
}

export type SettingsUpdater = (prev: UserSettings) => UserSettings;

/**
 * The autosave engine behind {@link useUserSettings}, kept free of React so the
 * ordering rules below are unit-testable without a DOM.
 *
 * Two invariants make concurrent writes safe, and both are easy to lose:
 *
 * 1. **Replay, don't overwrite.** A queued edit is stored as its *updater*, then
 *    replayed against a fresh read of the file at write time — never applied to
 *    a copy loaded earlier. The terminal's font-size zoom and the Board's
 *    group-by toggle read-modify-write the same `agentboard` block from
 *    elsewhere in the app, so writing a stale whole object would revert whichever
 *    of them landed in between.
 * 2. **One write at a time.** Flushes chain on `tail`. Without that, flipping two
 *    toggles quickly races: the second flush reads the file before the first's
 *    write lands, and its read-modify-write silently reverts the first change.
 *
 * `deferMs` debounces `queue(fn, { defer: true })` — used for anything typed, so
 * a half-finished path template or token isn't written (and handed to the live
 * scheduler) mid-keystroke.
 */
export function createSettingsWriter({
  load,
  save,
  onState,
  deferMs = DEFER_MS,
}: {
  load: () => Promise<UserSettings | null>;
  save: (settings: UserSettings) => Promise<boolean>;
  onState: (state: SaveState) => void;
  deferMs?: number;
}) {
  let queued: SettingsUpdater[] = [];
  let timer: ReturnType<typeof setTimeout> | null = null;
  let tail: Promise<void> = Promise.resolve();

  const drain = async () => {
    const replay = queued;
    if (replay.length === 0) return;
    queued = [];
    onState("saving");
    const disk = await load();
    if (!disk) {
      onState("error");
      return;
    }
    onState((await save(replay.reduce((acc, fn) => fn(acc), disk))) ? "saved" : "error");
  };

  const flush = (): Promise<void> => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    const run = tail.then(drain);
    // Swallow only to keep the chain alive for the next write; `onState` has
    // already reported the failure.
    tail = run.catch(() => {});
    return run;
  };

  return {
    queue(fn: SettingsUpdater, opts?: { defer?: boolean }) {
      queued.push(fn);
      if (!opts?.defer) {
        void flush();
        return;
      }
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => void flush(), deferMs);
    },
    flush,
  };
}

/**
 * Load the settings once and persist every edit as it happens — there is no
 * explicit save.
 *
 * `update` applies an updater to the on-screen copy for instant feedback and
 * hands the same updater to {@link createSettingsWriter}, which owns the write
 * ordering. Pass `{ defer: true }` for anything typed, and call `flush` on blur
 * to commit it without waiting out the debounce; toggles and selects omit it and
 * save on the spot.
 *
 * `settings` is `null` until loaded and stays `null` in browser dev where the
 * command returns `null`; edits are dropped rather than queued in that case,
 * since there's no file to merge into.
 */
export function useUserSettings() {
  const [settings, setSettings] = useState<UserSettings | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const live = useRef(false);
  const [writer] = useState(() =>
    createSettingsWriter({
      load: loadUserSettings,
      save: saveUserSettings,
      onState: setSaveState,
    }),
  );

  useEffect(() => {
    let alive = true;
    void loadUserSettings().then((s) => {
      if (!alive) return;
      live.current = s !== null;
      setSettings(s);
      setLoaded(true);
    });
    return () => {
      alive = false;
    };
  }, []);

  // Commit a still-pending debounce on unmount, so an edit made and immediately
  // navigated away from isn't lost. Post-unmount `setSaveState` calls are no-ops,
  // which is fine — the write is the part that matters.
  useEffect(
    () => () => {
      void writer.flush();
    },
    [writer],
  );

  const update = useCallback(
    (fn: SettingsUpdater, opts?: { defer?: boolean }) => {
      if (!live.current) return;
      setSettings((prev) => (prev ? fn(prev) : prev));
      writer.queue(fn, opts);
    },
    [writer],
  );

  return { settings, loaded, saveState, update, flush: writer.flush };
}
