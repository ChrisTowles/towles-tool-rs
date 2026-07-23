import { useEffect, useMemo } from "react";
import { AppHeader } from "@/components/app-header";
import { AppSidebar, AppSidebarIcons } from "@/components/app-sidebar";
import { CommandPalette } from "@/components/command-palette";
import { DayBar } from "@/components/day-bar";
import { DmBanner } from "@/components/dm-banner";
import { ErrorBoundary } from "@/components/error-boundary";
import { MonacoDialogHost } from "@/components/monaco-dialog-host";
import { QuickLog } from "@/components/quick-log";
import { StatusBar } from "@/components/status-bar";
import { ResumePicker } from "@/components/resume-picker";
import { UpdateBanner } from "@/components/update-banner";
import { ZenIndicator } from "@/components/zen-indicator";
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AgentboardStateProvider } from "@/lib/agentboard-state";
import { NowProvider } from "@/lib/now";
import { StoreSnapshotProvider } from "@/lib/store-snapshot";
import { SCREENS } from "@/lib/screens";
import { ShortcutHelpHost, useShortcuts, type ShortcutScope } from "@/lib/shortcuts";
import { WorkspaceProvider, useWorkspace } from "@/lib/workspace";
import { SCREEN_COMPONENTS } from "@/screens";

/** Global (always-active) bindings + the `?` help overlay. Screen-scoped
 * bindings live with their screens (e.g. Agentboard), gated on their tab. */
function Shortcuts() {
  const {
    setPaletteOpen,
    toggleSidebar,
    toggleZen,
    paletteOpen,
    activeTab,
    openTabs,
    openTab,
    openSettingsTab,
    closeTab,
  } = useWorkspace();

  useShortcuts(
    useMemo(() => {
      const handlers: Partial<Record<string, () => void>> = {
        palette: () => setPaletteOpen(!paletteOpen),
        settings: () => openSettingsTab(),
        sidebar: toggleSidebar,
        zen: toggleZen,
        quicklog: () => window.dispatchEvent(new Event("quicklog:open")),
        "close-tab": () => closeTab(activeTab),
        "next-tab": () => {
          const idx = openTabs.indexOf(activeTab);
          if (idx !== -1) openTab(openTabs[(idx + 1) % openTabs.length]);
        },
        "prev-tab": () => {
          const idx = openTabs.indexOf(activeTab);
          if (idx !== -1) openTab(openTabs[(idx - 1 + openTabs.length) % openTabs.length]);
        },
      };
      // Register only the tab-jump bindings that map to an open tab, so an
      // unused digit falls through instead of being swallowed as a no-op.
      openTabs.slice(0, 9).forEach((id, i) => {
        handlers[`tab-${i + 1}`] = () => openTab(id);
      });
      return handlers;
    }, [
      setPaletteOpen,
      toggleSidebar,
      toggleZen,
      paletteOpen,
      activeTab,
      openTabs,
      openTab,
      openSettingsTab,
      closeTab,
    ]),
    activeTab,
  );

  const activeScopes: ShortcutScope[] =
    activeTab === "agentboard"
      ? ["global", "agentboard"]
      : activeTab === "board"
        ? ["global", "board"]
        : ["global"];
  return <ShortcutHelpHost activeScopes={activeScopes} screen={activeTab} />;
}

function Workspace() {
  const { openTabs, activeTab, sidebarCollapsed, zen, setZen, paletteOpen } = useWorkspace();

  // Escape exits zen — but only when nothing else is claiming Escape. An open
  // dialog/palette (Radix `role="dialog"` with `data-state="open"`, plus the
  // paletteOpen flag) handles its own Escape to close first; we don't steal it.
  useEffect(() => {
    if (!zen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (paletteOpen) return;
      if (document.querySelector('[role="dialog"][data-state="open"]')) return;
      e.preventDefault();
      setZen(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [zen, paletteOpen, setZen]);

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      {!zen && <AppHeader />}
      {!zen && <DayBar />}
      <UpdateBanner />
      <DmBanner />

      <div className="flex min-h-0 flex-1">
        {/* Icon-collapsed nav: a fixed-width strip outside the panel group, not
            a ResizablePanel — its width is locked, so react-resizable-panels'
            drag machinery would be pure dead weight (mirrors the Agentboard
            rail's RailIconStrip). */}
        {!zen && sidebarCollapsed && (
          <div className="w-12 shrink-0 border-r bg-background">
            <AppSidebarIcons />
          </div>
        )}
        <ResizablePanelGroup orientation="horizontal" className="min-h-0 flex-1">
          {!zen && !sidebarCollapsed && (
            <>
              <ResizablePanel defaultSize="190px" minSize="160px" maxSize="240px">
                <AppSidebar />
              </ResizablePanel>
              <ResizableHandle />
            </>
          )}
          <ResizablePanel key="main">
            <div className="flex h-full flex-col">
              <div className="min-h-0 flex-1">
                {openTabs.map((id) => {
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

      {!zen && <StatusBar />}

      {zen && <ZenIndicator onExit={() => setZen(false)} />}
      <Shortcuts />
      <CommandPalette />
      <QuickLog />
      <ResumePicker />
      {/* The VS Code layer can raise a confirmation from any pane (or none),
          so its host lives at the root. */}
      <MonacoDialogHost />
      <Toaster />
    </div>
  );
}

export function App() {
  return (
    <WorkspaceProvider>
      <NowProvider>
        <StoreSnapshotProvider>
          <AgentboardStateProvider>
            <TooltipProvider>
              <Workspace />
            </TooltipProvider>
          </AgentboardStateProvider>
        </StoreSnapshotProvider>
      </NowProvider>
    </WorkspaceProvider>
  );
}
