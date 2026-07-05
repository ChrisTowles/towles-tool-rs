import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ScreenId } from "@/lib/screens";

type WorkspaceState = {
  visited: ScreenId[];
  activeTab: ScreenId;
  sidebarVisible: boolean;
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
  const [sidebarVisible, setSidebarVisible] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);

  const openTab = useCallback((id: ScreenId) => {
    setVisited((prev) => (prev.includes(id) ? prev : [...prev, id]));
    setActiveTab(id);
  }, []);

  const toggleSidebar = useCallback(() => setSidebarVisible((v) => !v), []);

  const value = useMemo(
    () => ({
      visited,
      activeTab,
      sidebarVisible,
      paletteOpen,
      openTab,
      toggleSidebar,
      setPaletteOpen,
    }),
    [visited, activeTab, sidebarVisible, paletteOpen, openTab, toggleSidebar],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) throw new Error("useWorkspace must be used within WorkspaceProvider");
  return ctx;
}
