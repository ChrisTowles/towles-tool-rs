import { useEffect, useRef, useState } from "react";
import { invoke } from "@/lib/tauri";
import { uiAction } from "@/lib/ui-action";
import type { StatePayload } from "@/lib/agentboard";
import { RAIL_COLLAPSE_KEY } from "./helpers";

export type CollapseState = {
  /** The persisted collapse map (repo/folder row keys + the rail sentinel). */
  collapsed: Record<string, boolean>;
  /** Flip one entry and persist it incrementally. */
  toggleCollapsed: (key: string) => void;
  /** Set (rather than flip) one entry — used by arrow-key navigation. */
  setCollapsedTo: (key: string, next: boolean) => void;
  /** Whether the whole rail is collapsed to its icon strip. */
  railCollapsed: boolean;
  /** Toggle the whole-rail icon collapse (emits its ui.action). */
  toggleRail: () => void;
};

/**
 * Folder-rail collapse/expand state (issue #52): hydrated once from
 * `ab_get_state`, then this local copy is the live truth — same pattern as
 * `wins`, except each toggle saves incrementally (one key at a time) rather
 * than a debounced whole-blob save, since a collapse entry is never ambiguous
 * between "not yet toggled" and "explicitly reset".
 */
export function useCollapseState(state: StatePayload): CollapseState {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const hydratedCollapsed = useRef(false);
  useEffect(() => {
    if (!hydratedCollapsed.current && state.ts > 0) {
      hydratedCollapsed.current = true;
      setCollapsed(state.collapsed);
    }
  }, [state.ts, state.collapsed]);

  function toggleCollapsed(key: string) {
    setCollapsed((c) => {
      const next = !c[key];
      void invoke("ab_save_collapsed", { key, collapsed: next });
      return { ...c, [key]: next };
    });
  }

  // Set (rather than flip) one collapse-map entry — used by arrow-key
  // navigation, where left always means collapsed and right always means
  // expanded regardless of the current state.
  function setCollapsedTo(key: string, next: boolean) {
    setCollapsed((c) => {
      if (!!c[key] === next) return c;
      void invoke("ab_save_collapsed", { key, collapsed: next });
      return { ...c, [key]: next };
    });
  }

  // Whole-rail icon collapse (issue #70): same persisted map, sentinel key.
  const railCollapsed = !!collapsed[RAIL_COLLAPSE_KEY];
  const toggleRail = () => {
    uiAction("agentboard.rail_toggle", "agentboard", railCollapsed ? "expand" : "collapse");
    toggleCollapsed(RAIL_COLLAPSE_KEY);
  };

  return { collapsed, toggleCollapsed, setCollapsedTo, railCollapsed, toggleRail };
}
