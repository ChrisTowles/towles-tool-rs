import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke, isTauri } from "./tauri";
import {
  EMPTY_SNAPSHOT,
  mockSnapshot,
  toStoreSnapshot,
  type StoreSnapshot,
  type WireStoreSnapshot,
} from "./data";

/**
 * A single app-wide subscription to the live store snapshot. Every screen stays
 * mounted (App.tsx toggles `hidden` rather than unmounting), so before this
 * provider each of ~14 consumers registered its own `store://snapshot` listener
 * and re-parsed the wire payload independently — one event meant ~14 parses.
 * The provider subscribes once and shares the parsed value through context,
 * modeled on {@link NowProvider} (`lib/now.tsx`).
 *
 * Until the real store answers, the snapshot is empty and `live` is false;
 * outside Tauri entirely (plain-Vite browser dev) it falls back to
 * {@link mockSnapshot}.
 */
type StoreSnapshotValue = { snapshot: StoreSnapshot; live: boolean };

const StoreSnapshotContext = createContext<StoreSnapshotValue | null>(null);

export function StoreSnapshotProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<StoreSnapshot>(EMPTY_SNAPSHOT);
  const [live, setLive] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    if (!isTauri()) {
      // Browser dev: render mock rows so screens are visually workable.
      setSnapshot(mockSnapshot());
      return;
    }
    // A `store://snapshot` event can beat the initial `store_snapshot` invoke;
    // once one has, its data is fresher, so don't let the invoke roll it back.
    let eventArrived = false;

    void (async () => {
      try {
        const { listen } = await import("@tauri-apps/api/event");

        const sub = await listen<WireStoreSnapshot>("store://snapshot", (e) => {
          eventArrived = true;
          setSnapshot(toStoreSnapshot(e.payload));
          setLive(true);
        });
        if (disposed) {
          sub();
          return;
        }
        unlisten = sub;
      } catch {
        // Event bridge not ready — stay on the empty snapshot.
        return;
      }

      const initial = await invoke<WireStoreSnapshot>("store_snapshot");
      if (initial.isOk() && !disposed && !eventArrived) {
        setSnapshot(toStoreSnapshot(initial.value));
        setLive(true);
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const value = useMemo<StoreSnapshotValue>(() => ({ snapshot, live }), [snapshot, live]);
  return <StoreSnapshotContext.Provider value={value}>{children}</StoreSnapshotContext.Provider>;
}

/**
 * The live store snapshot and whether it's backed by the real store yet, shared
 * across the app from a single subscription. Must be used under a
 * {@link StoreSnapshotProvider}.
 */
export function useStoreSnapshot(): StoreSnapshotValue {
  const ctx = useContext(StoreSnapshotContext);
  if (ctx === null) {
    throw new Error("useStoreSnapshot must be used within a StoreSnapshotProvider");
  }
  return ctx;
}
