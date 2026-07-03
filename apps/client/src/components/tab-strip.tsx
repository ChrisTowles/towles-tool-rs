import { X } from "lucide-react";
import { SCREENS } from "@/lib/screens";
import { useWorkspace } from "@/lib/workspace";
import { cn } from "@/lib/utils";

export function TabStrip() {
  const { tabs, activeTab, setActiveTab, closeTab } = useWorkspace();

  if (tabs.length === 0) return null;

  return (
    <div className="flex shrink-0 items-center overflow-x-auto overflow-y-hidden border-b">
      {tabs.map((id) => {
        const screen = SCREENS[id];
        const active = id === activeTab;
        return (
          <div
            key={id}
            role="tab"
            aria-selected={active}
            tabIndex={0}
            className={cn(
              "group relative flex cursor-default select-none items-center gap-1.5 border-r px-3 py-1.5 text-sm",
              active
                ? "text-foreground after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:bg-primary"
                : "text-muted-foreground hover:bg-muted/50 hover:text-foreground",
            )}
            onClick={() => setActiveTab(id)}
            onKeyDown={(e) => e.key === "Enter" && setActiveTab(id)}
            onAuxClick={(e) => e.button === 1 && closeTab(id)}
          >
            <screen.icon className="size-3.5 text-muted-foreground" />
            {screen.title}
            <button
              aria-label={`Close ${screen.title}`}
              className={cn(
                "rounded-sm p-0.5 hover:bg-muted",
                active ? "opacity-60 hover:opacity-100" : "opacity-0 group-hover:opacity-60",
              )}
              onClick={(e) => {
                e.stopPropagation();
                closeTab(id);
              }}
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
