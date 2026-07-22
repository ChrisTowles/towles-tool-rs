import { useCallback, useEffect, useState } from "react";
import { SETTINGS_SAVED_EVENT, loadUserSettings, saveUserSettings } from "./settings";

/** Built-in default for `agentboard.hideInactiveRepos` — off, showing everything. */
export const DEFAULT_HIDE_INACTIVE_REPOS = false;

/**
 * Track the Agentboard rail's "hide inactive repos" eye-icon filter
 * (`agentboard.hideInactiveRepos`) as state, plus a setter that updates state
 * and persists back to the shared settings file. Like `useBoardGroupByRepo`
 * (`lib/board-prefs.ts`), re-reads on `SETTINGS_SAVED_EVENT` and on window
 * focus so a change made elsewhere flows back into this hook.
 */
export function useHideInactiveRepos(): [boolean, (on: boolean) => void] {
  const [hideInactive, setHideInactive] = useState(DEFAULT_HIDE_INACTIVE_REPOS);
  useEffect(() => {
    let alive = true;
    const load = () =>
      void loadUserSettings().then((s) => {
        if (alive && s)
          setHideInactive(s.agentboard?.hideInactiveRepos ?? DEFAULT_HIDE_INACTIVE_REPOS);
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
    setHideInactive(on);
    void loadUserSettings().then((s) => {
      if (!s) return;
      void saveUserSettings({
        ...s,
        agentboard: { ...s.agentboard, hideInactiveRepos: on },
      });
    });
  }, []);

  return [hideInactive, persist];
}
