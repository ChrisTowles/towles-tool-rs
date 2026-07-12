import { X } from "lucide-react";
import { SCREENS } from "@/lib/screens";
import { shortcutHint } from "@/lib/shortcuts";
import { cn } from "@/lib/utils";
import { useWorkspace } from "@/lib/workspace";

/**
 * The strip of open (mounted) screens above the content area. The sidebar opens
 * tabs; this bar shows what's open, lets you switch between them, and — via the
 * per-tab ✕ — unmount them so the `visited` set doesn't grow without bound.
 * ⌘/Ctrl+1…9 jump to a tab by position; ⌘/Ctrl+W closes the active one.
 */
export function TabBar() {
  const { visited, activeTab, openTab, closeTab } = useWorkspace();
  // The last tab can't be closed, so its ✕ would be a dead control — hide it.
  const canClose = visited.length > 1;

  return (
    <div className="flex h-9 shrink-0 items-stretch gap-0.5 overflow-x-auto border-b bg-background px-1">
      {visited.map((id, i) => {
        const screen = SCREENS[id];
        const active = id === activeTab;
        const hint = i < 9 ? shortcutHint(`tab-${i + 1}`) : undefined;
        return (
          <div
            key={id}
            className={cn(
              "group my-1 flex items-center rounded-md pl-2 text-sm",
              active
                ? "bg-accent text-accent-foreground"
                : "text-muted-foreground hover:bg-accent/50",
              !canClose && "pr-2",
            )}
          >
            <button
              type="button"
              onClick={() => openTab(id)}
              aria-current={active || undefined}
              title={hint ? `${screen.title} (${hint})` : screen.title}
              className="flex items-center gap-1.5 py-1"
            >
              <screen.icon className="size-3.5 shrink-0" />
              <span className="whitespace-nowrap">{screen.title}</span>
            </button>
            {canClose && (
              <button
                type="button"
                aria-label={`Close ${screen.title}`}
                onClick={() => closeTab(id)}
                className="ml-1 mr-1 rounded-sm p-0.5 text-muted-foreground opacity-0 hover:bg-accent hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
              >
                <X className="size-3" />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
