import { DotCount } from "@/components/agentboard-bits";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  agentRollup,
  rollupAlertColor,
  rollupAlertTextColor,
  useAgentboardState,
} from "@/lib/agentboard";
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
                  aria-current={active || undefined}
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
                      {rollup.busy > 0 && <DotCount status="busy" n={rollup.busy} />}
                      {rollup.waiting > 0 && <DotCount status="waiting" n={rollup.waiting} />}
                      {rollup.error > 0 && <DotCount status="error" n={rollup.error} />}
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

/** The outer nav collapsed to a narrow icon strip (issue #70's sibling ask):
 * one icon per screen, sections separated by hairlines, active screen gets
 * the violet left border. The one count the expanded nav shows today — the
 * Agentboard running-agent rollup — rides along as a corner badge, so
 * collapsing the sidebar never hides "something needs you". */
export function AppSidebarIcons() {
  const { activeTab, openTab } = useWorkspace();
  const state = useAgentboardState();
  const rollup = agentRollup(state.repos, Date.now(), state.compactRecommendPercent);
  const badgeColor = rollupAlertColor(rollup);

  return (
    <ScrollArea className="h-full">
      <div className="flex flex-col items-center gap-1 py-2">
        {NAV_SECTIONS.map((section, i) => (
          <div key={section.label} className="flex flex-col items-center gap-1">
            {i > 0 && <div className="my-1 h-px w-6 bg-border" />}
            {section.screens.map((id) => {
              const screen = SCREENS[id];
              const active = activeTab === id;
              const showBadge = id === "agentboard" && rollup.total > 0;
              return (
                <Tooltip key={id}>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      aria-label={screen.title}
                      aria-current={active || undefined}
                      onClick={() => openTab(id)}
                      className={cn(
                        "relative flex size-9 shrink-0 items-center justify-center rounded-md border-l-2 border-transparent text-muted-foreground hover:bg-accent/50",
                        active && "border-l-violet-500 bg-accent text-foreground",
                      )}
                    >
                      <screen.icon className="size-4" />
                      {showBadge && (
                        <span
                          className={cn(
                            "absolute -right-1 -bottom-1 min-w-4 rounded-full px-0.5 text-center font-mono text-[9px] leading-[14px]",
                            badgeColor,
                            rollupAlertTextColor(badgeColor),
                          )}
                        >
                          {rollup.total}
                        </span>
                      )}
                    </button>
                  </TooltipTrigger>
                  <TooltipContent side="right">
                    {screen.title}
                    {showBadge &&
                      ` — ${rollup.total} agent${rollup.total === 1 ? "" : "s"}${rollup.waiting > 0 ? `, ${rollup.waiting} waiting` : ""}${rollup.error > 0 ? `, ${rollup.error} errored` : ""}`}
                  </TooltipContent>
                </Tooltip>
              );
            })}
          </div>
        ))}
      </div>
    </ScrollArea>
  );
}
