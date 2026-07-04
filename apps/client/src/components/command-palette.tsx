import { Moon, PanelLeft, PenLine, Settings, Sun } from "lucide-react";
import {
  Command,
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { useTheme } from "@/components/theme-provider";
import { SCREENS } from "@/lib/screens";
import { useWorkspace } from "@/lib/workspace";

export function CommandPalette() {
  const { paletteOpen, setPaletteOpen, openTab, toggleSidebar, setSettingsOpen } = useWorkspace();
  const { theme, setTheme } = useTheme();

  const resolvedDark =
    theme === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
      : theme === "dark";

  const run = (action: () => void) => {
    setPaletteOpen(false);
    action();
  };

  return (
    <CommandDialog open={paletteOpen} onOpenChange={setPaletteOpen}>
      <Command>
        <CommandInput placeholder="Search screens and actions…" />
        <CommandList>
        <CommandEmpty>Nothing matches.</CommandEmpty>
        <CommandGroup heading="Go to">
          {Object.values(SCREENS).map((screen) => (
            <CommandItem
              key={screen.id}
              keywords={screen.keywords}
              onSelect={() => run(() => openTab(screen.id))}
            >
              <screen.icon />
              {screen.title}
            </CommandItem>
          ))}
        </CommandGroup>
        <CommandSeparator />
        <CommandGroup heading="Actions">
          <CommandItem
            keywords={["journal", "log", "note", "today"]}
            onSelect={() => run(() => window.dispatchEvent(new Event("quicklog:open")))}
          >
            <PenLine />
            Journal: log a line
            <CommandShortcut>⌘J</CommandShortcut>
          </CommandItem>
          <CommandItem
            keywords={["theme", "dark", "light"]}
            onSelect={() => run(() => setTheme(resolvedDark ? "light" : "dark"))}
          >
            {resolvedDark ? <Sun /> : <Moon />}
            Switch to {resolvedDark ? "light" : "dark"} theme
          </CommandItem>
          <CommandItem
            keywords={["sidebar", "panel"]}
            onSelect={() => run(toggleSidebar)}
          >
            <PanelLeft />
            Toggle sidebar
            <CommandShortcut>⌘B</CommandShortcut>
          </CommandItem>
          <CommandItem
            keywords={["settings", "preferences"]}
            onSelect={() => run(() => setSettingsOpen(true))}
          >
            <Settings />
            Open settings
            <CommandShortcut>⌘,</CommandShortcut>
          </CommandItem>
        </CommandGroup>
        </CommandList>
      </Command>
    </CommandDialog>
  );
}
