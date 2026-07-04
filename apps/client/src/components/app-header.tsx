import { PanelLeft, Search, Settings } from "lucide-react";
import { ThemeToggle } from "@/components/theme-toggle";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useAppSlot } from "@/lib/data";
import { openSettings } from "@/lib/open-settings";
import { useWorkspace } from "@/lib/workspace";

/**
 * Fixed palette of literal Tailwind classes (so the JIT sees them) — one per
 * slot window, picked by hashing the slot name so a given checkout always keeps
 * the same accent.
 */
const SLOT_COLORS = [
  { badge: "border-blue-500/40 bg-blue-500/10 text-blue-700 dark:text-blue-300", dot: "bg-blue-500" },
  {
    badge: "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    dot: "bg-emerald-500",
  },
  {
    badge: "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300",
    dot: "bg-amber-500",
  },
  {
    badge: "border-violet-500/40 bg-violet-500/10 text-violet-700 dark:text-violet-300",
    dot: "bg-violet-500",
  },
  { badge: "border-rose-500/40 bg-rose-500/10 text-rose-700 dark:text-rose-300", dot: "bg-rose-500" },
  { badge: "border-cyan-500/40 bg-cyan-500/10 text-cyan-700 dark:text-cyan-300", dot: "bg-cyan-500" },
];

function slotColor(slot: string) {
  let hash = 0;
  for (let i = 0; i < slot.length; i++) hash = (hash * 31 + slot.charCodeAt(i)) | 0;
  return SLOT_COLORS[Math.abs(hash) % SLOT_COLORS.length];
}

/** Strip the shared prefix so the badge reads "slot-2", not the whole repo name. */
function slotShortName(slot: string): string {
  const m = slot.match(/slot-\w+$/i);
  return m ? m[0] : slot;
}

function SlotBadge() {
  const slot = useAppSlot();
  if (!slot) return null;
  const color = slotColor(slot);
  return (
    <Badge variant="outline" className={color.badge} title={slot}>
      <span className={`size-2 rounded-full ${color.dot}`} />
      {slotShortName(slot)}
    </Badge>
  );
}

export function AppHeader() {
  const { toggleSidebar, setPaletteOpen } = useWorkspace();

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
            onClick={() => void openSettings()}
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
