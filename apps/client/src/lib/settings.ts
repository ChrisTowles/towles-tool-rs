import { useCallback, useEffect, useState } from "react";
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

export type CalendarCollector = {
  enabled: boolean;
  provider: string;
  refreshMinutes: number;
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
   * unset = on), and `copyOnSelect` (terminal
   * copies the selection to the clipboard on selection end; unset = off).
   */
  agentboard?: {
    notifyNeedsYou?: boolean;
    notifyMeetingStart?: boolean;
    notifyReviewRequested?: boolean;
    notifyChecksFailed?: boolean;
    copyOnSelect?: boolean;
  } & Record<string, unknown>;
};

export type SaveState = "idle" | "saving" | "saved" | "error";

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
    void invokeCmd<UserSettings>("settings_get").then((s) => {
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
    } catch {
      setSaveState("error");
    }
  }, [settings]);

  return { settings, loaded, saveState, update, save };
}
