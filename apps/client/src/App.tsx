import { useMemo } from "react";
import { AppHeader } from "@/components/app-header";
import { AppSidebar } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { DayBar } from "@/components/day-bar";
import { ErrorBoundary } from "@/components/error-boundary";
import { QuickLog } from "@/components/quick-log";
import { StatusBar } from "@/components/status-bar";
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
  const { setPaletteOpen, toggleSidebar, paletteOpen, activeTab } = useWorkspace();

  useShortcuts(
    useMemo(
      () => ({
        palette: () => setPaletteOpen(!paletteOpen),
        settings: () => void openSettings(),
        sidebar: toggleSidebar,
        quicklog: () => window.dispatchEvent(new Event("quicklog:open")),
      }),
      [setPaletteOpen, toggleSidebar, paletteOpen],
    ),
  );

  const activeScopes: ShortcutScope[] =
    activeTab === "agentboard" ? ["global", "agentboard"] : ["global"];
  return <ShortcutHelpHost activeScopes={activeScopes} />;
}

function Workspace() {
  const { visited, activeTab, sidebarVisible } = useWorkspace();

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <AppHeader />
      <DayBar />

      <ResizablePanelGroup
        key={sidebarVisible ? "with-sidebar" : "no-sidebar"}
        orientation="horizontal"
        className="min-h-0 flex-1"
      >
        {sidebarVisible && (
          <>
            <ResizablePanel defaultSize="220px" minSize="160px" maxSize="260px">
              <AppSidebar />
            </ResizablePanel>
            <ResizableHandle />
          </>
        )}
        <ResizablePanel>
          <div className="h-full">
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
        </ResizablePanel>
      </ResizablePanelGroup>

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
