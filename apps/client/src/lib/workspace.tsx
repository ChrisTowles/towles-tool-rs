import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ScreenId } from "@/lib/screens";
import { focusTargetStore, type FocusTarget } from "@/lib/focus-target";
import { settingsTargetStore, type SettingsTarget } from "@/lib/settings-target";
import {
  ACTIVE_TAB_KEY,
  loadWorkspaceTabs,
  VISITED_TABS_KEY,
} from "@/lib/workspace-persistence";

type WorkspaceState = {
  visited: ScreenId[];
  /** Screens in most-recently-opened order (front = most recent), for the
   * command palette's "Recent" section. Distinct from `visited`, which is
   * first-visit order and drives which screens stay mounted. */
  recent: ScreenId[];
  activeTab: ScreenId;
  /** Whole outer nav collapsed to an icon-only strip (mirrors the Agentboard
   * rail's icon collapse) — never fully hidden, so a screen is always one
   * click away. */
  sidebarCollapsed: boolean;
  /** Zen focus mode: the chrome (sidebar, day bar, tab bar) is hidden so the
   * active screen owns the whole window — the literal get-in-the-zone gesture.
   * Not persisted: relaunch always comes back with the chrome shown. */
  zen: boolean;
  paletteOpen: boolean;
  openTab: (id: ScreenId) => void;
  /** Open (mounting if needed) the target's screen and stash a one-shot focus
   * request, so the destination screen scrolls that row into view and flashes
   * it. See {@link FocusTarget}. */
  openTabWithFocus: (target: FocusTarget) => void;
  /** Open the Settings tab, optionally deep-linked onto a sub-tab and/or a
   * prefilled filter (e.g. `{ tab: "collectors", filter: "slack" }`). See
   * {@link SettingsTarget}. */
  openSettingsTab: (target?: SettingsTarget) => void;
  /** Unmount a screen (remove it from `visited`). The last remaining tab can't
   * be closed — some screen must always be shown. Closing the active tab moves
   * focus to the neighbor that slides into its place. */
  closeTab: (id: ScreenId) => void;
  toggleSidebar: () => void;
  toggleZen: () => void;
  setZen: (on: boolean) => void;
  setPaletteOpen: (open: boolean) => void;
};

const WorkspaceContext = createContext<WorkspaceState | null>(null);

const SIDEBAR_COLLAPSED_KEY = "tt-sidebar-collapsed";

// Read once at module load: the tab the app was left on last relaunch, with
// cockpit as the cold-start fallback (see loadWorkspaceTabs).
const restored = loadWorkspaceTabs(
  localStorage.getItem(ACTIVE_TAB_KEY),
  localStorage.getItem(VISITED_TABS_KEY),
);

export function WorkspaceProvider({ children }: { children: React.ReactNode }) {
  // Screens are mounted once on first visit and kept mounted (hidden via CSS)
  // so their local state — e.g. Agentboard's terminals — survives switching.
  const [visited, setVisited] = useState<ScreenId[]>(restored.visited);
  // Seed "recent" from the restored tabs, freshest (the active tab) first.
  const [recent, setRecent] = useState<ScreenId[]>(() => [
    restored.activeTab,
    ...restored.visited.filter((id) => id !== restored.activeTab),
  ]);
  const [activeTab, setActiveTab] = useState<ScreenId>(restored.activeTab);
  // Icon-only is the default; expanding is the remembered opt-in.
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) !== "false",
  );
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Deliberately in-memory only: zen is a per-moment gesture, not a preference,
  // so a relaunch always restores the full chrome.
  const [zen, setZen] = useState(false);

  // Persist the active tab and the visited (mounted) set so relaunch restores
  // where you were. Persisting `visited` too keeps a closed tab from
  // resurrecting: closeTab drops it here, so it isn't rebuilt on reload.
  useEffect(() => {
    localStorage.setItem(ACTIVE_TAB_KEY, activeTab);
    localStorage.setItem(VISITED_TABS_KEY, JSON.stringify(visited));
  }, [activeTab, visited]);

  const openTab = useCallback((id: ScreenId) => {
    setVisited((prev) => (prev.includes(id) ? prev : [...prev, id]));
    setRecent((prev) => [id, ...prev.filter((x) => x !== id)]);
    setActiveTab(id);
  }, []);

  const openTabWithFocus = useCallback(
    (target: FocusTarget) => {
      openTab(target.screen);
      focusTargetStore.set(target);
    },
    [openTab],
  );

  const openSettingsTab = useCallback(
    (target?: SettingsTarget) => {
      openTab("settings");
      if (target) settingsTargetStore.set(target);
    },
    [openTab],
  );

  const closeTab = useCallback(
    (id: ScreenId) => {
      // Never close the last tab — a screen is always shown.
      if (visited.length <= 1 || !visited.includes(id)) return;
      const idx = visited.indexOf(id);
      const next = visited.filter((s) => s !== id);
      setVisited(next);
      if (activeTab === id) {
        // Slide focus to the tab that takes this one's slot (or the new last).
        setActiveTab(next[Math.min(idx, next.length - 1)]);
      }
    },
    [visited, activeTab],
  );

  const toggleSidebar = useCallback(
    () =>
      setSidebarCollapsed((v) => {
        localStorage.setItem(SIDEBAR_COLLAPSED_KEY, String(!v));
        return !v;
      }),
    [],
  );

  const toggleZen = useCallback(() => setZen((v) => !v), []);

  const value = useMemo(
    () => ({
      visited,
      recent,
      activeTab,
      sidebarCollapsed,
      zen,
      paletteOpen,
      openTab,
      openTabWithFocus,
      openSettingsTab,
      closeTab,
      toggleSidebar,
      toggleZen,
      setZen,
      setPaletteOpen,
    }),
    [
      visited,
      recent,
      activeTab,
      sidebarCollapsed,
      zen,
      paletteOpen,
      openTab,
      openTabWithFocus,
      openSettingsTab,
      closeTab,
      toggleSidebar,
      toggleZen,
    ],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) throw new Error("useWorkspace must be used within WorkspaceProvider");
  return ctx;
}
