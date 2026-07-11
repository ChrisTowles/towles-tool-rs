import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ScreenId } from "@/lib/screens";

type WorkspaceState = {
  visited: ScreenId[];
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

export function WorkspaceProvider({ children }: { children: React.ReactNode }) {
  // Screens are mounted once on first visit and kept mounted (hidden via CSS)
  // so their local state — e.g. Agentboard's terminals — survives switching.
  const [visited, setVisited] = useState<ScreenId[]>(["cockpit"]);
  const [activeTab, setActiveTab] = useState<ScreenId>("cockpit");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);

  const openTab = useCallback((id: ScreenId) => {
    setVisited((prev) => (prev.includes(id) ? prev : [...prev, id]));
    setActiveTab(id);
  }, []);

  const toggleSidebar = useCallback(() => setSidebarCollapsed((v) => !v), []);

  const value = useMemo(
    () => ({
      visited,
      activeTab,
      sidebarCollapsed,
      paletteOpen,
      openTab,
      toggleSidebar,
      setPaletteOpen,
    }),
    [visited, activeTab, sidebarCollapsed, paletteOpen, openTab, toggleSidebar],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) throw new Error("useWorkspace must be used within WorkspaceProvider");
  return ctx;
}
