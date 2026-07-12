import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

/**
 * A single app-wide wall clock. One `setInterval` feeds every countdown,
 * age readout, and escalation timer (day bar, header clock, DM banner,
 * Cockpit, PRs, config) instead of each mounting its own ticker.
 *
 * Default 15s granularity: fine enough for the DM banner's 5/10-minute
 * escalation thresholds, calm enough for minute-resolution countdowns
 * elsewhere. A consumer that needs finer resolution — e.g. the Cockpit's
 * `m:ss` countdown in the final two minutes before a meeting — asks for it
 * via {@link useNowInterval}; the provider then ticks at the fastest interval
 * any live consumer requests, so there is still only one clock.
 */
const DEFAULT_TICK_MS = 15_000;

type NowValue = {
  now: number;
  /** Register a requested tick interval; returns an unregister fn. */
  requestInterval: (ms: number) => () => void;
};

const NowContext = createContext<NowValue | null>(null);

export function NowProvider({ children }: { children: ReactNode }) {
  const [now, setNow] = useState(() => Date.now());
  const [tickMs, setTickMs] = useState(DEFAULT_TICK_MS);
  const requests = useRef<number[]>([]);

  const requestInterval = useCallback((ms: number) => {
    requests.current.push(ms);
    setTickMs(Math.min(...requests.current, DEFAULT_TICK_MS));
    return () => {
      const i = requests.current.indexOf(ms);
      if (i !== -1) requests.current.splice(i, 1);
      setTickMs(Math.min(...requests.current, DEFAULT_TICK_MS));
    };
  }, []);

  useEffect(() => {
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), tickMs);
    return () => clearInterval(id);
  }, [tickMs]);

  const value = useMemo<NowValue>(() => ({ now, requestInterval }), [now, requestInterval]);
  return <NowContext.Provider value={value}>{children}</NowContext.Provider>;
}

function useNowContext(): NowValue {
  const ctx = useContext(NowContext);
  if (ctx === null) {
    throw new Error("useNow must be used within a NowProvider");
  }
  return ctx;
}

/** Current wall-clock time (epoch ms), shared across the app. Refreshed at the
 * shared tick — 15s by default, faster while a consumer holds a
 * {@link useNowInterval} request. Must be used under a {@link NowProvider}. */
export function useNow(): number {
  return useNowContext().now;
}

/** Ask the shared clock to tick at least this fast while this component is
 * mounted (pass `undefined` to request nothing). Lets a screen sharpen the
 * clock — e.g. 1s for a `m:ss` countdown — without spinning up its own ticker.
 * The provider drops back to the default once the last requester unmounts. */
export function useNowInterval(intervalMs: number | undefined): void {
  const { requestInterval } = useNowContext();
  useEffect(() => {
    if (intervalMs === undefined) return;
    return requestInterval(intervalMs);
  }, [intervalMs, requestInterval]);
}
