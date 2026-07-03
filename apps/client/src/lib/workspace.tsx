import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ScreenId } from "@/lib/screens";

type WorkspaceState = {
  tabs: ScreenId[];
  activeTab: ScreenId | null;
  sidebarVisible: boolean;
  paletteOpen: boolean;
  settingsOpen: boolean;
  openTab: (id: ScreenId) => void;
  closeTab: (id: ScreenId) => void;
  setActiveTab: (id: ScreenId) => void;
  toggleSidebar: () => void;
  setPaletteOpen: (open: boolean) => void;
  setSettingsOpen: (open: boolean) => void;
};

const WorkspaceContext = createContext<WorkspaceState | null>(null);

export function WorkspaceProvider({ children }: { children: React.ReactNode }) {
  const [tabs, setTabs] = useState<ScreenId[]>(["journal-today"]);
  const [activeTab, setActiveTab] = useState<ScreenId | null>("journal-today");
  const [sidebarVisible, setSidebarVisible] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const openTab = useCallback((id: ScreenId) => {
    setTabs((prev) => (prev.includes(id) ? prev : [...prev, id]));
    setActiveTab(id);
  }, []);

  const closeTab = useCallback((id: ScreenId) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t !== id);
      setActiveTab((active) => {
        if (active !== id) return active;
        const index = prev.indexOf(id);
        return next[Math.min(index, next.length - 1)] ?? null;
      });
      return next;
    });
  }, []);

  const toggleSidebar = useCallback(() => setSidebarVisible((v) => !v), []);

  const value = useMemo(
    () => ({
      tabs,
      activeTab,
      sidebarVisible,
      paletteOpen,
      settingsOpen,
      openTab,
      closeTab,
      setActiveTab,
      toggleSidebar,
      setPaletteOpen,
      setSettingsOpen,
    }),
    [tabs, activeTab, sidebarVisible, paletteOpen, settingsOpen, openTab, closeTab, toggleSidebar],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) throw new Error("useWorkspace must be used within WorkspaceProvider");
  return ctx;
}
