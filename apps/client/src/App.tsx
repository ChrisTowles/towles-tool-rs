import { useEffect } from "react";
import { Terminal } from "lucide-react";
import { AppHeader } from "@/components/app-header";
import { AppSidebar } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { DayBar } from "@/components/day-bar";
import { QuickLog } from "@/components/quick-log";
import { SettingsDialog } from "@/components/settings-dialog";
import { StatusBar } from "@/components/status-bar";
import { TabStrip } from "@/components/tab-strip";
import { Kbd } from "@/components/ui/kbd";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SCREENS } from "@/lib/screens";
import { WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { SCREEN_COMPONENTS } from "@/screens";

function EmptyState() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
      <Terminal className="size-10" />
      <p className="text-sm">No open tabs.</p>
      <p className="text-sm">
        Press <Kbd>⌘K</Kbd> to search, or pick a screen from the sidebar.
      </p>
    </div>
  );
}

function Shortcuts() {
  const { setPaletteOpen, setSettingsOpen, toggleSidebar, closeTab, activeTab, paletteOpen } =
    useWorkspace();

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
          setSettingsOpen(true);
          break;
        case "b":
          e.preventDefault();
          toggleSidebar();
          break;
        case "w":
          if (activeTab) {
            e.preventDefault();
            closeTab(activeTab);
          }
          break;
        case "j":
          e.preventDefault();
          window.dispatchEvent(new Event("quicklog:open"));
          break;
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setPaletteOpen, setSettingsOpen, toggleSidebar, closeTab, activeTab, paletteOpen]);

  return null;
}

function Workspace() {
  const { tabs, activeTab, sidebarVisible } = useWorkspace();

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
            <ResizablePanel defaultSize="220px" minSize="160px" maxSize="400px">
              <AppSidebar />
            </ResizablePanel>
            <ResizableHandle />
          </>
        )}
        <ResizablePanel>
          <div className="flex h-full flex-col">
            <TabStrip />
            <div className="min-h-0 flex-1">
              {activeTab ? (
                tabs.map((id) => {
                  const Screen = SCREEN_COMPONENTS[id];
                  const hidden = id !== activeTab;
                  // Keep inactive tabs mounted so their local state survives switching.
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
                })
              ) : (
                <EmptyState />
              )}
            </div>
          </div>
        </ResizablePanel>
      </ResizablePanelGroup>

      <StatusBar />

      <Shortcuts />
      <CommandPalette />
      <QuickLog />
      <SettingsDialog />
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
