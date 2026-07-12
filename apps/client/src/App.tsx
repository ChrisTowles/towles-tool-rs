import { useMemo } from "react";
import { AppHeader } from "@/components/app-header";
import { AppSidebar, AppSidebarIcons } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { DayBar } from "@/components/day-bar";
import { DmBanner } from "@/components/dm-banner";
import { ErrorBoundary } from "@/components/error-boundary";
import { QuickLog } from "@/components/quick-log";
import { StatusBar } from "@/components/status-bar";
import { TabBar } from "@/components/tab-bar";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { openSettings } from "@/lib/open-settings";
import { SCREENS } from "@/lib/screens";
import { ShortcutHelpHost, useShortcuts, type ShortcutScope } from "@/lib/shortcuts";
import { WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { SCREEN_COMPONENTS } from "@/screens";

/** Global (always-active) bindings + the `?` help overlay. Screen-scoped
 * bindings live with their screens (e.g. Agentboard), gated on their tab. */
function Shortcuts() {
  const { setPaletteOpen, toggleSidebar, paletteOpen, activeTab, visited, openTab, closeTab } =
    useWorkspace();

  useShortcuts(
    useMemo(() => {
      const handlers: Partial<Record<string, () => void>> = {
        palette: () => setPaletteOpen(!paletteOpen),
        settings: () => void openSettings(),
        sidebar: toggleSidebar,
        quicklog: () => window.dispatchEvent(new Event("quicklog:open")),
        "close-tab": () => closeTab(activeTab),
      };
      // Register only the tab-jump bindings that map to an open tab, so an
      // unused digit falls through instead of being swallowed as a no-op.
      visited.slice(0, 9).forEach((id, i) => {
        handlers[`tab-${i + 1}`] = () => openTab(id);
      });
      return handlers;
    }, [setPaletteOpen, toggleSidebar, paletteOpen, activeTab, visited, openTab, closeTab]),
  );

  const activeScopes: ShortcutScope[] =
    activeTab === "agentboard" ? ["global", "agentboard"] : ["global"];
  return <ShortcutHelpHost activeScopes={activeScopes} />;
}

function Workspace() {
  const { visited, activeTab, sidebarCollapsed } = useWorkspace();

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <AppHeader />
      <DayBar />
      <DmBanner />

      <div className="flex min-h-0 flex-1">
        {/* Icon-collapsed nav: a fixed-width strip outside the panel group, not
            a ResizablePanel — its width is locked, so react-resizable-panels'
            drag machinery would be pure dead weight (mirrors the Agentboard
            rail's RailIconStrip). */}
        {sidebarCollapsed && (
          <div className="w-12 shrink-0 border-r bg-background">
            <AppSidebarIcons />
          </div>
        )}
        <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
          {!sidebarCollapsed && (
            <>
              <ResizablePanel defaultSize="190px" minSize="160px" maxSize="240px">
                <AppSidebar />
              </ResizablePanel>
              <ResizableHandle />
            </>
          )}
          <ResizablePanel key="main">
            <div className="flex h-full flex-col">
              <TabBar />
              <div className="min-h-0 flex-1">
                {visited.map((id) => {
                  const Screen = SCREEN_COMPONENTS[id];
                  const hidden = id !== activeTab;
                  // Keep visited screens mounted so their local state survives switching.
                  // Full-bleed screens (e.g. terminals) skip the centered, scrolling
                  // content wrapper and own the whole content area.
                  return SCREENS[id].fullBleed ? (
                    <div key={id} hidden={hidden} className="h-full">
                      <ErrorBoundary label={SCREENS[id].title}>
                        <Screen />
                      </ErrorBoundary>
                    </div>
                  ) : (
                    <ScrollArea key={id} hidden={hidden} className="h-full">
                      <div className="mx-auto max-w-3xl p-6">
                        <ErrorBoundary label={SCREENS[id].title}>
                          <Screen />
                        </ErrorBoundary>
                      </div>
                    </ScrollArea>
                  );
                })}
              </div>
            </div>
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>

      <StatusBar />

      <Shortcuts />
      <CommandPalette />
      <QuickLog />
      <Toaster />
    </div>
  );
}

export function App() {
  return (
    <WorkspaceProvider>
      <TooltipProvider>
        <Workspace />
      </TooltipProvider>
    </WorkspaceProvider>
  );
}
