import { useCallback, useEffect, useState } from "react";
import { UserSettingsSchema } from "./schemas/settings";
import { invokeCmd, invokeOrThrow } from "./tauri";

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

export type CalendarCollector = {
  enabled: boolean;
  provider: string;
  refreshMinutes: number;
  quietHours: CalendarQuietHours;
};

export type PrCollector = {
  enabled: boolean;
  refreshSeconds: number;
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
   * unset = on).
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
  } & Record<string, unknown>;
};

export type SaveState = "idle" | "saving" | "saved" | "error";

/** Fired on `window` after a successful `settings_set` — lets other in-app
 * consumers of a settings value (e.g. terminal prefs, the shortcuts registry)
 * refresh live instead of waiting for a `"focus"` event that no longer fires
 * when Settings is just a tab in the same window. */
export const SETTINGS_SAVED_EVENT = "tt:settings-saved";

/**
 * Load the settings once, edit a local draft, and persist it. `update` takes an
 * immutable updater so nested edits stay simple; `save` writes the whole draft
 * (the backend merge preserves unknown keys). `settings` is `null` until loaded
 * and stays `null` in browser dev where the command returns `null`.
 */
export function useUserSettings() {
  const [settings, setSettings] = useState<UserSettings | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saveState, setSaveState] = useState<SaveState>("idle");

  useEffect(() => {
    let alive = true;
    void invokeCmd<UserSettings>("settings_get", {}, UserSettingsSchema).then((s) => {
      if (alive) {
        setSettings(s);
        setLoaded(true);
      }
    });
    return () => {
      alive = false;
    };
  }, []);

  const update = useCallback((fn: (prev: UserSettings) => UserSettings) => {
    setSettings((prev) => (prev ? fn(prev) : prev));
    setSaveState("idle");
  }, []);

  const save = useCallback(async () => {
    if (!settings) return;
    setSaveState("saving");
    try {
      await invokeOrThrow("settings_set", { settings });
      setSaveState("saved");
      window.dispatchEvent(new Event(SETTINGS_SAVED_EVENT));
    } catch {
      setSaveState("error");
    }
  }, [settings]);

  return { settings, loaded, saveState, update, save };
}
