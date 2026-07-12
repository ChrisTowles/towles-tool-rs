import { useEffect, useRef, type RefObject } from "react";
import { invokeCmd } from "./tauri";
import type { UserSettings } from "./settings";

/** Built-in default for `agentboard.copyOnSelect` — off, matching tt-config. */
export const DEFAULT_COPY_ON_SELECT = false;

/**
 * Track the `agentboard.copyOnSelect` preference in a ref the terminal's render
 * effect can read live without re-subscribing. Settings live in a separate OS
 * window, so a save there won't push into this window; instead we re-read on
 * window focus, which is when the user returns from the Settings window.
 */
export function useCopyOnSelect(): RefObject<boolean> {
  const ref = useRef(DEFAULT_COPY_ON_SELECT);
  useEffect(() => {
    let alive = true;
    const load = () =>
      void invokeCmd<UserSettings>("settings_get").then((s) => {
        if (alive && s) ref.current = s.agentboard?.copyOnSelect ?? DEFAULT_COPY_ON_SELECT;
      });
    load();
    window.addEventListener("focus", load);
    return () => {
      alive = false;
      window.removeEventListener("focus", load);
    };
  }, []);
  return ref;
}
