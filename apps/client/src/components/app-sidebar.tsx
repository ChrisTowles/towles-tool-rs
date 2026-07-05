import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { agentRollup, useAgentboardState } from "@/lib/agentboard";
import { NAV_SECTIONS, SCREENS } from "@/lib/screens";
import { useWorkspace } from "@/lib/workspace";
import { cn } from "@/lib/utils";

export function AppSidebar() {
  const { activeTab, openTab } = useWorkspace();
  const state = useAgentboardState();
  // Coldness flips at most once per TTL; re-render on each state event is enough.
  const rollup = agentRollup(state.repos, Date.now(), state.compactRecommendPercent);

  return (
    <ScrollArea className="h-full">
      <nav className="flex flex-col gap-4 p-2">
        {NAV_SECTIONS.map((section) => (
          <div key={section.label} className="flex flex-col gap-0.5">
            <div className="px-2 pb-1 text-xs font-medium text-muted-foreground">
              {section.label}
            </div>
            {section.screens.map((id) => {
              const screen = SCREENS[id];
              const active = activeTab === id;
              return (
                <Button
                  key={id}
                  variant="ghost"
                  size="sm"
                  className={cn(
                    "justify-start font-normal",
                    active && "bg-accent text-accent-foreground",
                  )}
                  onClick={() => openTab(id)}
                >
                  <screen.icon className="text-muted-foreground" />
                  {screen.title}
                  {id === "agentboard" && rollup.total > 0 && (
                    <span className="ml-auto flex items-center gap-1.5 font-mono text-[10.5px] text-muted-foreground">
                      {rollup.total}
                      {rollup.busy > 0 && <MiniDot className="bg-yellow-500" n={rollup.busy} />}
                      {rollup.waiting > 0 && <MiniDot className="bg-blue-500" n={rollup.waiting} />}
                      {rollup.error > 0 && <MiniDot className="bg-red-500" n={rollup.error} />}
                      {rollup.compact > 0 && (
                        <span className="text-sky-500" title="cold sessions worth compacting">
                          ❄{rollup.compact}
                        </span>
                      )}
                    </span>
                  )}
                </Button>
              );
            })}
          </div>
        ))}
      </nav>
    </ScrollArea>
  );
}

/** A status-colored micro-dot + count, e.g. "●3", for the nav rollup. */
function MiniDot({ className, n }: { className: string; n: number }) {
  return (
    <span className="flex items-center gap-0.5">
      <span className={cn("size-1.5 rounded-full", className)} />
      {n}
    </span>
  );
}
