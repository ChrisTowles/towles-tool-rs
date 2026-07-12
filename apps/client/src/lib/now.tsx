import { createContext, useContext, useEffect, useState, type ReactNode } from "react";

/**
 * A single app-wide wall clock. One `setInterval` feeds every countdown,
 * age readout, and escalation timer (day bar, header clock, DM banner,
 * Cockpit, PRs, config) instead of each mounting its own ticker.
 *
 * 15s granularity: fine enough for the DM banner's 5/10-minute escalation
 * thresholds, calm enough for minute-resolution countdowns elsewhere.
 */
const TICK_MS = 15_000;

const NowContext = createContext<number | null>(null);

export function NowProvider({ children }: { children: ReactNode }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(id);
  }, []);
  return <NowContext.Provider value={now}>{children}</NowContext.Provider>;
}

/** Current wall-clock time (epoch ms), shared across the app and refreshed
 * every {@link TICK_MS}. Must be used under a {@link NowProvider}. */
export function useNow(): number {
  const now = useContext(NowContext);
  if (now === null) {
    throw new Error("useNow must be used within a NowProvider");
  }
  return now;
}
