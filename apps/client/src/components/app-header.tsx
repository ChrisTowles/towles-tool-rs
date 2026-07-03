import { PanelLeft, Search, Settings } from "lucide-react";
import { ThemeToggle } from "@/components/theme-toggle";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useWorkspace } from "@/lib/workspace";

export function AppHeader() {
  const { toggleSidebar, setPaletteOpen, setSettingsOpen } = useWorkspace();

  return (
    <header className="flex h-11 shrink-0 items-center gap-2 border-b px-2">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-sm" aria-label="Toggle sidebar" onClick={toggleSidebar}>
            <PanelLeft />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          Toggle sidebar <Kbd>⌘B</Kbd>
        </TooltipContent>
      </Tooltip>

      <h1 className="font-heading px-1 text-sm font-semibold">Towles Tool</h1>

      <div className="flex-1" />

      <Button
        variant="outline"
        size="sm"
        className="w-56 justify-between text-muted-foreground"
        onClick={() => setPaletteOpen(true)}
      >
        <span className="flex items-center gap-2">
          <Search className="size-3.5" />
          Search…
        </span>
        <Kbd>⌘K</Kbd>
      </Button>

      <ThemeToggle />

      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Open settings"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          Settings <Kbd>⌘,</Kbd>
        </TooltipContent>
      </Tooltip>
    </header>
  );
}
