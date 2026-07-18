import { useCallback, useEffect, type DependencyList } from "react";

/**
 * Wrap an async loader in a stable callback and fire it once whenever `deps`
 * changes (typically `[]` for mount-only, or a value like a screen's `ty`
 * prop that should trigger a refetch when it changes). Returns the same
 * stable function so callers can also invoke it manually — e.g. a Refresh
 * button, or after a mutation — without re-subscribing the effect.
 */
export function useAsyncRefresh(load: () => Promise<void>, deps: DependencyList): () => void {
  // eslint-disable-next-line react-hooks/exhaustive-deps -- deps is caller-supplied, not statically analyzable
  const stable = useCallback(load, deps);
  useEffect(() => {
    void stable();
  }, [stable]);
  return stable;
}
