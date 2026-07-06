import { useEffect } from "react";
import { AppHeader } from "@/components/app-header";
import { AppSidebar } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { DayBar } from "@/components/day-bar";
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
import { WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { SCREEN_COMPONENTS } from "@/screens";

function Shortcuts() {
  const { setPaletteOpen, toggleSidebar, paletteOpen } = useWorkspace();

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return;
      switch (e.key) {
        case "k":
          e.preventDefault();
          setPaletteOpen(!paletteOpen);
          break;
        case ",":
          e.preventDefault();
          void openSettings();
          break;
        case "b":
          e.preventDefault();
          toggleSidebar();
          break;
        case "j":
          e.preventDefault();
          window.dispatchEvent(new Event("quicklog:open"));
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setPaletteOpen, toggleSidebar, paletteOpen]);

  return null;
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
                  <Screen />
                </div>
              ) : (
                <ScrollArea key={id} hidden={hidden} className="h-full">
                  <div className="mx-auto max-w-3xl p-6">
                    <Screen />
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
