import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { invokeCmd, invokeOrThrow } from "./tauri";
import type { UserSettings } from "./settings";

/** Built-in default for `agentboard.copyOnSelect` — on, matching tt-config. */
export const DEFAULT_COPY_ON_SELECT = true;

/** Built-in default for `agentboard.terminalFontSize` (px), matching tt-config. */
export const DEFAULT_TERMINAL_FONT_SIZE = 13;
/** Terminal font-size zoom bounds — small enough to stay legible, large enough
 * to fit a usable grid. */
export const MIN_TERMINAL_FONT_SIZE = 8;
export const MAX_TERMINAL_FONT_SIZE = 32;

/** Clamp/round an arbitrary px value into the supported terminal font range. */
export function clampTerminalFontSize(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_TERMINAL_FONT_SIZE;
  return Math.max(MIN_TERMINAL_FONT_SIZE, Math.min(MAX_TERMINAL_FONT_SIZE, Math.round(px)));
}

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

/**
 * Track the terminal font size (`agentboard.terminalFontSize`) as state so the
 * canvas render effect can key on it and re-measure the cell grid on change,
 * plus a setter that clamps, updates state, and persists back to the shared
 * settings file. Like {@link useCopyOnSelect}, we re-read on window focus so a
 * change made in the separate Settings window flows back into this window.
 */
export function useTerminalFontSize(): [number, (px: number) => void] {
  const [fontSize, setFontSize] = useState(DEFAULT_TERMINAL_FONT_SIZE);
  useEffect(() => {
    let alive = true;
    const load = () =>
      void invokeCmd<UserSettings>("settings_get").then((s) => {
        if (alive && s)
          setFontSize(
            clampTerminalFontSize(s.agentboard?.terminalFontSize ?? DEFAULT_TERMINAL_FONT_SIZE),
          );
      });
    load();
    window.addEventListener("focus", load);
    return () => {
      alive = false;
      window.removeEventListener("focus", load);
    };
  }, []);

  // Persist a zoom change back to the shared settings file. Read-modify-write
  // the whole settings object so the TS CLI's unknown keys survive the save.
  const persist = useCallback((px: number) => {
    const clamped = clampTerminalFontSize(px);
    setFontSize(clamped);
    void invokeCmd<UserSettings>("settings_get").then((s) => {
      if (!s) return;
      void invokeOrThrow("settings_set", {
        settings: { ...s, agentboard: { ...s.agentboard, terminalFontSize: clamped } },
      }).catch(() => {});
    });
  }, []);

  return [fontSize, persist];
}
