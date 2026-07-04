import { PanelLeft, Search, Settings } from "lucide-react";
import { ThemeToggle } from "@/components/theme-toggle";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useAppSlot } from "@/lib/data";
import { useWorkspace } from "@/lib/workspace";

/** Deterministic hue from the slot name so each window gets its own accent. */
function slotHue(slot: string): number {
  let hash = 0;
  for (let i = 0; i < slot.length; i++) hash = (hash * 31 + slot.charCodeAt(i)) | 0;
  return Math.abs(hash) % 360;
}

/** Strip the shared prefix so the badge reads "slot-2", not the whole repo name. */
function slotShortName(slot: string): string {
  const m = slot.match(/slot-\w+$/i);
  return m ? m[0] : slot;
}

function SlotBadge() {
  const slot = useAppSlot();
  if (!slot) return null;
  const hue = slotHue(slot);
  return (
    <Badge
      variant="outline"
      style={{
        borderColor: `hsl(${hue} 60% 45% / 0.5)`,
        backgroundColor: `hsl(${hue} 60% 45% / 0.12)`,
      }}
      title={slot}
    >
      <span className="size-2 rounded-full" style={{ backgroundColor: `hsl(${hue} 65% 50%)` }} />
      {slotShortName(slot)}
    </Badge>
  );
}

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

      <SlotBadge />

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
