import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { invoke } from "./tauri";
import type { StatePayload, WindowsPayload } from "./agentboard";

const EMPTY_WINDOWS: WindowsPayload = { windows: [], activeWindows: {} };

const EMPTY: StatePayload = {
  repos: [],
  preferredEditor: "",
  compactRecommendPercent: 30,
  windows: EMPTY_WINDOWS,
  collapsed: {},
  ts: 0,
};

/**
 * A single app-wide subscription to the live agentboard state. Screens stay
 * mounted (App.tsx toggles `hidden`), so before this provider each of ~5
 * consumers ran its own `ab_get_state` fetch + `agentboard://state` listener.
 * The provider subscribes once and shares the payload through context, the
 * same pattern as {@link StoreSnapshotProvider} / {@link NowProvider}.
 *
 * Returns the latest payload (empty until the first snapshot arrives).
 */
const AgentboardStateContext = createContext<StatePayload | null>(null);

export function AgentboardStateProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<StatePayload>(EMPTY);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      // Outside Tauri (bare-browser dev), `listen` throws on the missing IPC
      // internals — stay on the empty state instead of leaking unhandled
      // rejections.
      if (!("__TAURI_INTERNALS__" in window)) {
        setState(EMPTY);
        return;
      }

      const { listen } = await import("@tauri-apps/api/event");

      // Every payload is stamped with its compute time (`ts`) — never let an
      // older snapshot replace a newer one. The initial `ab_get_state` fetch
      // below resolves *after* the subscription is live, so a debounced
      // `agentboard://state` event (e.g. the one `ab_add_repo` triggers during
      // task creation) can land first; without the guard the slower fetch
      // would roll the rail back to a snapshot that predates it.
      const accept = (payload: StatePayload) =>
        setState((cur) => (payload.ts < cur.ts ? cur : payload));

      const sub = await listen<StatePayload>("agentboard://state", (e) => {
        accept(e.payload);
      });
      if (disposed) {
        sub();
        return;
      }
      unlisten = sub;

      const initial = await invoke<StatePayload>("ab_get_state");
      if (initial.isOk() && !disposed) accept(initial.value);
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return (
    <AgentboardStateContext.Provider value={state}>{children}</AgentboardStateContext.Provider>
  );
}

/**
 * The live agentboard state, shared across the app from a single subscription.
 * Empty until the first snapshot arrives. Must be used under an
 * {@link AgentboardStateProvider}.
 */
export function useAgentboardState(): StatePayload {
  const ctx = useContext(AgentboardStateContext);
  if (ctx === null) {
    throw new Error("useAgentboardState must be used within an AgentboardStateProvider");
  }
  return ctx;
}
