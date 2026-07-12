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
      toggleSidebar,
      setPaletteOpen,
    }),
    [visited, recent, activeTab, sidebarCollapsed, paletteOpen, openTab, toggleSidebar],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) throw new Error("useWorkspace must be used within WorkspaceProvider");
  return ctx;
}
