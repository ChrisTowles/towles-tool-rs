import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { NAV_SECTIONS, SCREENS } from "@/lib/screens";
import { useWorkspace } from "@/lib/workspace";
import { cn } from "@/lib/utils";

export function AppSidebar() {
  const { activeTab, openTab } = useWorkspace();

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
                </Button>
              );
            })}
          </div>
        ))}
      </nav>
    </ScrollArea>
  );
}
