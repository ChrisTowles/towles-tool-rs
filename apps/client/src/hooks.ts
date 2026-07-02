import { useEffect, useRef, useState } from "react";
import { SPINNER_INTERVAL_MS } from "./lib/constants";

/** A wall-clock that re-renders every `intervalMs` (drives elapsed/cache/loop). */
export function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

/**
 * Monotonic spinner index, advancing every 120ms ONLY while `active` (any agent
 * running). Freezes — and does not reset — when nothing is running.
 */
export function useSpinner(active: boolean): number {
  const [idx, setIdx] = useState(0);
  const ref = useRef(idx);
  ref.current = idx;
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setIdx((i) => i + 1), SPINNER_INTERVAL_MS);
    return () => clearInterval(id);
  }, [active]);
  return idx;
}
