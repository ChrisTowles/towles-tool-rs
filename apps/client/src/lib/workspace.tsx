import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ScreenId } from "@/lib/screens";

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
  paletteOpen: boolean;
  openTab: (id: ScreenId) => void;
  /** Unmount a screen (remove it from `visited`). The last remaining tab can't
   * be closed — some screen must always be shown. Closing the active tab moves
   * focus to the neighbor that slides into its place. */
  closeTab: (id: ScreenId) => void;
  toggleSidebar: () => void;
  setPaletteOpen: (open: boolean) => void;
};

const WorkspaceContext = createContext<WorkspaceState | null>(null);

const SIDEBAR_COLLAPSED_KEY = "tt-sidebar-collapsed";

export function WorkspaceProvider({ children }: { children: React.ReactNode }) {
  // Screens are mounted once on first visit and kept mounted (hidden via CSS)
  // so their local state — e.g. Agentboard's terminals — survives switching.
  const [visited, setVisited] = useState<ScreenId[]>(["cockpit"]);
  const [recent, setRecent] = useState<ScreenId[]>(["cockpit"]);
  const [activeTab, setActiveTab] = useState<ScreenId>("cockpit");
  // Icon-only is the default; expanding is the remembered opt-in.
  const [sidebarCollapsed, setSidebarCollapsed] = useState(
    () => localStorage.getItem(SIDEBAR_COLLAPSED_KEY) !== "false",
  );
  const [paletteOpen, setPaletteOpen] = useState(false);

  const openTab = useCallback((id: ScreenId) => {
    setVisited((prev) => (prev.includes(id) ? prev : [...prev, id]));
    setRecent((prev) => [id, ...prev.filter((x) => x !== id)]);
    setActiveTab(id);
  }, []);

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

  const value = useMemo(
    () => ({
      visited,
      recent,
      activeTab,
      sidebarCollapsed,
      paletteOpen,
      openTab,
      closeTab,
      toggleSidebar,
      setPaletteOpen,
    }),
    [visited, recent, activeTab, sidebarCollapsed, paletteOpen, openTab, closeTab, toggleSidebar],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) throw new Error("useWorkspace must be used within WorkspaceProvider");
  return ctx;
}
