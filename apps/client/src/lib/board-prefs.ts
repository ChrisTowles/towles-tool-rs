import { useCallback, useEffect, useState } from "react";
import { SETTINGS_SAVED_EVENT, loadUserSettings, saveUserSettings } from "./settings";

/** Built-in default for `agentboard.boardGroupByRepo` — grouped, today's look. */
export const DEFAULT_BOARD_GROUP_BY_REPO = true;

/**
 * Track the Board's group-by-repo-swimlane preference
 * (`agentboard.boardGroupByRepo`) as state, plus a setter that updates state
 * and persists back to the shared settings file. Like `useTerminalFontSize`
 * (`lib/terminal-prefs.ts`), re-reads on `SETTINGS_SAVED_EVENT` and on window
 * focus so a change made elsewhere flows back into this hook.
 */
export function useBoardGroupByRepo(): [boolean, (on: boolean) => void] {
  const [groupByRepo, setGroupByRepo] = useState(DEFAULT_BOARD_GROUP_BY_REPO);
  useEffect(() => {
    let alive = true;
    const load = () =>
      void loadUserSettings().then((s) => {
        if (alive && s)
          setGroupByRepo(s.agentboard?.boardGroupByRepo ?? DEFAULT_BOARD_GROUP_BY_REPO);
      });
    load();
    window.addEventListener("focus", load);
    window.addEventListener(SETTINGS_SAVED_EVENT, load);
    return () => {
      alive = false;
      window.removeEventListener("focus", load);
      window.removeEventListener(SETTINGS_SAVED_EVENT, load);
    };
  }, []);

  // Read-modify-write the whole settings object so unknown keys survive the
  // save. Best-effort: a failed persist leaves this session's view correct.
  const persist = useCallback((on: boolean) => {
    setGroupByRepo(on);
    void loadUserSettings().then((s) => {
      if (!s) return;
      void saveUserSettings({
        ...s,
        agentboard: { ...s.agentboard, boardGroupByRepo: on },
      });
    });
  }, []);

  return [groupByRepo, persist];
}
